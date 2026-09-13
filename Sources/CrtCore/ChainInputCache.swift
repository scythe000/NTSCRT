import Foundation
import Metal

/// Chain inputs (NTSC stage baked in, downscaled) kept per video frame, so a
/// clip that has played once — or been pre-rendered while paused — plays
/// back with no decode-to-NTSC work at all: the After Effects RAM-preview
/// model. Entries are small because the chain input is the *downscale*
/// resolution (~600 KB at 320 px), so a typical 10 s clip is ~150 MB.
///
/// Main-thread owned, except `isCached`, which the playback producer calls
/// from its own thread to skip the NTSC step for frames already here.
public final class ChainInputCache: @unchecked Sendable {

    /// What an entry was rendered with; a lookup under a different stamp is
    /// a miss. The generation covers NTSC settings, the NTSC toggle, the
    /// timeline and the downscale settings (all bump it); the downscale
    /// spec rides along explicitly as a belt-and-braces check.
    public struct Stamp: Equatable, Sendable {
        public var generation: Int
        public var downscale: DownscaleSpec?
        public init(generation: Int, downscale: DownscaleSpec?) {
            self.generation = generation
            self.downscale = downscale
        }
    }

    private struct Entry {
        let texture: MTLTexture
        let stamp: Stamp
        let bytes: Int
    }

    /// Byte budget. Beyond it, insertion is refused and the clip streams
    /// live from there — no eviction, because a loop that doesn't fit would
    /// otherwise evict exactly the frames it needs next.
    public let capacity: Int
    public private(set) var bytes = 0
    private var entries: [Int: Entry] = [:]

    private let probeLock = NSLock()
    private var probe: [Int: Int] = [:]     // frame → generation, producer-readable

    /// A quarter of physical memory, at most 1 GiB.
    public static var defaultCapacity: Int {
        min(1 << 30, Int(ProcessInfo.processInfo.physicalMemory / 4))
    }

    public init(capacity: Int = ChainInputCache.defaultCapacity) {
        self.capacity = capacity
    }

    public static func byteCount(width: Int, height: Int) -> Int { width * height * 4 }

    public var count: Int { entries.count }

    public func count(matching stamp: Stamp) -> Int {
        entries.values.reduce(0) { $0 + ($1.stamp == stamp ? 1 : 0) }
    }

    public func hasRoom(for byteCount: Int) -> Bool { bytes + byteCount <= capacity }

    public func lookup(frame: Int, stamp: Stamp) -> MTLTexture? {
        guard let e = entries[frame], e.stamp == stamp else { return nil }
        return e.texture
    }

    /// False when the budget is exhausted (nothing stored). Replacing a
    /// frame's existing entry is always allowed when the new one is no larger.
    @discardableResult
    public func insert(frame: Int, texture: MTLTexture, stamp: Stamp) -> Bool {
        let n = Self.byteCount(width: texture.width, height: texture.height)
        let old = entries[frame]?.bytes ?? 0
        guard bytes - old + n <= capacity else { return false }
        entries[frame] = Entry(texture: texture, stamp: stamp, bytes: n)
        bytes += n - old
        probeLock.lock(); probe[frame] = stamp.generation; probeLock.unlock()
        return true
    }

    public func invalidateAll() {
        entries.removeAll()
        bytes = 0
        probeLock.lock(); probe.removeAll(); probeLock.unlock()
    }

    /// Any thread: is `frame` held for `generation`? Cheap enough for the
    /// producer to ask per frame.
    public func isCached(frame: Int, generation: Int) -> Bool {
        probeLock.lock(); defer { probeLock.unlock() }
        return probe[frame] == generation
    }

    /// Contiguous runs of cached frames under `stamp`, for a render bar.
    public func cachedRanges(stamp: Stamp) -> [Range<Int>] {
        let frames = entries.filter { $0.value.stamp == stamp }.keys.sorted()
        var out: [Range<Int>] = []
        for f in frames {
            if let last = out.last, last.upperBound == f {
                out[out.count - 1] = last.lowerBound..<(f + 1)
            } else {
                out.append(f..<(f + 1))
            }
        }
        return out
    }
}
