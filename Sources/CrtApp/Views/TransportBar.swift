import SwiftUI

/// Full-width video transport docked under the preview: play/pause, a frame
/// scrubber spanning the pane (maximum scrubbing precision), and a
/// frame/time/fps readout. Rendered only when a video source is loaded.
struct TransportBar: View {
    @Environment(AppState.self) private var state

    var body: some View {
        if let vs = state.videoSource {
            let total = vs.totalFrames
            let idx = Binding<Double>(
                get: { Double(state.currentFrameIndex) },
                set: { state.currentFrameIndex = Int($0.rounded()) }
            )
            HStack(spacing: 10) {
                Button {
                    state.togglePlayback()
                } label: {
                    Image(systemName: state.videoPlaying ? "pause.fill" : "play.fill")
                        .font(.system(size: 13))
                        .frame(width: 18)
                }
                .buttonStyle(.borderless)
                .disabled(state.exportInProgress)
                .help(state.videoPlaying ? "Pause" : "Play the video in the preview (with all effects applied)")

                VStack(spacing: 1) {
                    Slider(value: idx, in: 0...Double(max(1, total - 1)), step: 1)
                    // RAM-preview render bar (see TimelineBar.cachedStrip);
                    // inset to roughly match the slider's thumb travel.
                    GeometryReader { g in
                        let inset: CGFloat = 8
                        let w = max(1, g.size.width - 2 * inset)
                        ForEach(state.cachedRanges, id: \.lowerBound) { r in
                            Rectangle()
                                .fill(Color.green.opacity(0.6))
                                .frame(width: max(1, CGFloat(r.count) / CGFloat(total) * w), height: 2)
                                .offset(x: inset + CGFloat(r.lowerBound) / CGFloat(total) * w)
                        }
                    }
                    .frame(height: 2)
                }

                Text("\(state.currentFrameIndex + 1)/\(total)  ·  \(String(format: "%.2fs", Double(state.currentFrameIndex) / Double(max(1, vs.frameRate))))  ·  \(String(format: "%.0f", vs.frameRate)) fps")
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .fixedSize()
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(.bar)
        }
    }
}
