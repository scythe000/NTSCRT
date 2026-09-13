import XCTest
import Metal
@testable import CrtCore

final class ChainInputCacheTests: XCTestCase {

    private var device: MTLDevice!

    override func setUpWithError() throws {
        device = try XCTUnwrap(MTLCreateSystemDefaultDevice())
    }

    private func texture(_ w: Int, _ h: Int) -> MTLTexture {
        let d = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: w, height: h, mipmapped: false)
        d.storageMode = .private
        return device.makeTexture(descriptor: d)!
    }

    private let spec = DownscaleSpec(width: 8, height: 4, method: .nearest)

    func testLookupHitsOnlyUnderTheSameStamp() {
        let cache = ChainInputCache(capacity: 1 << 20)
        let stamp = ChainInputCache.Stamp(generation: 3, downscale: spec)
        let t = texture(8, 4)
        XCTAssertTrue(cache.insert(frame: 5, texture: t, stamp: stamp))
        XCTAssertTrue(cache.lookup(frame: 5, stamp: stamp) === t)
        XCTAssertNil(cache.lookup(frame: 6, stamp: stamp))
        XCTAssertNil(cache.lookup(frame: 5, stamp: .init(generation: 4, downscale: spec)),
                     "a settings edit (new generation) must miss")
        let otherSpec = DownscaleSpec(width: 16, height: 8, method: .nearest)
        XCTAssertNil(cache.lookup(frame: 5, stamp: .init(generation: 3, downscale: otherSpec)),
                     "a different downscale must miss")
    }

    func testProducerProbeTracksGeneration() {
        let cache = ChainInputCache(capacity: 1 << 20)
        cache.insert(frame: 2, texture: texture(8, 4), stamp: .init(generation: 1, downscale: spec))
        XCTAssertTrue(cache.isCached(frame: 2, generation: 1))
        XCTAssertFalse(cache.isCached(frame: 2, generation: 2))
        XCTAssertFalse(cache.isCached(frame: 3, generation: 1))
        cache.invalidateAll()
        XCTAssertFalse(cache.isCached(frame: 2, generation: 1))
        XCTAssertEqual(cache.count, 0)
        XCTAssertEqual(cache.bytes, 0)
    }

    func testBudgetRefusesButNeverEvicts() {
        // Room for exactly two 8x4 BGRA frames (128 bytes each).
        let cache = ChainInputCache(capacity: 256)
        let stamp = ChainInputCache.Stamp(generation: 0, downscale: spec)
        XCTAssertTrue(cache.insert(frame: 0, texture: texture(8, 4), stamp: stamp))
        XCTAssertTrue(cache.insert(frame: 1, texture: texture(8, 4), stamp: stamp))
        XCTAssertFalse(cache.hasRoom(for: 128))
        XCTAssertFalse(cache.insert(frame: 2, texture: texture(8, 4), stamp: stamp))
        XCTAssertNotNil(cache.lookup(frame: 0, stamp: stamp), "earlier frames stay")
        XCTAssertEqual(cache.bytes, 256)
        // Replacing an existing frame with one of equal size is fine.
        XCTAssertTrue(cache.insert(frame: 1, texture: texture(8, 4), stamp: stamp))
        XCTAssertEqual(cache.bytes, 256)
    }

    func testCachedRangesMergeRunsAndFilterByStamp() {
        let cache = ChainInputCache(capacity: 1 << 20)
        let a = ChainInputCache.Stamp(generation: 0, downscale: spec)
        let b = ChainInputCache.Stamp(generation: 1, downscale: spec)
        for f in [0, 1, 2, 5, 6, 9] { cache.insert(frame: f, texture: texture(8, 4), stamp: a) }
        cache.insert(frame: 3, texture: texture(8, 4), stamp: b)
        XCTAssertEqual(cache.cachedRanges(stamp: a), [0..<3, 5..<7, 9..<10])
        XCTAssertEqual(cache.cachedRanges(stamp: b), [3..<4])
        XCTAssertEqual(cache.count(matching: a), 6)
    }
}
