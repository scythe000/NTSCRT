//! NTSCRT for Windows — entry point.
//!
//! Replaces `Sources/CrtApp/CrtAppApp.swift` (the SwiftUI `@main` App).

// Release builds are GUI subsystem apps: no console window flashes up
// alongside the window. Debug builds keep the console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("NTSCRT")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 600.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew {
                instance_descriptor: wgpu::InstanceDescriptor {
                    // D3D12 is the native Windows path; Vulkan is the
                    // fallback for adapters where it behaves better.
                    backends: wgpu::Backends::DX12 | wgpu::Backends::VULKAN,
                    ..wgpu::InstanceDescriptor::new_without_display_handle()
                },
                power_preference: wgpu::PowerPreference::HighPerformance,
                device_descriptor: std::sync::Arc::new(|adapter| {
                    // librashader caches compiled pipelines when asked, which
                    // needs PIPELINE_CACHE. Take it when the adapter offers
                    // it; the chain is told to skip the cache when it isn't
                    // available, since requesting it without the feature is a
                    // fatal wgpu validation error rather than a soft
                    // fallback.
                    let cache = adapter.features().contains(wgpu::Features::PIPELINE_CACHE);
                    wgpu::DeviceDescriptor {
                        label: Some("ntscrt"),
                        required_features: if cache {
                            wgpu::Features::PIPELINE_CACHE
                        } else {
                            wgpu::Features::empty()
                        },
                        // crt-royale is the heaviest preset in the bundle and
                        // needs more than the downlevel defaults allow.
                        required_limits: adapter.limits(),
                        experimental_features: wgpu::ExperimentalFeatures::disabled(),
                        memory_hints: wgpu::MemoryHints::Performance,
                        trace: wgpu::Trace::Off,
                    }
                }),
                display_handle: None,
                native_adapter_selector: None,
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(
        "NTSCRT",
        options,
        Box::new(|cc| Ok(Box::new(ntscrt_app::app::NtscrtApp::new(cc)))),
    )
}
