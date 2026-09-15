//! VHS-Studio — entry point.
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
            .with_title(format!("VHS-Studio {}", vhs_studio_app::about::BUILD.version))
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
                        label: Some("vhs-studio"),
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

    // A path on the command line opens as the source: that is what Windows
    // passes for "Open with" and for a file dropped onto the .exe, and it is
    // the counterpart of the macOS `CRT_SOURCE` hook for headless-launch
    // verification. `VHS_STUDIO_SOURCE` does the same from the environment.
    let initial_source = std::env::args_os()
        .nth(1)
        .or_else(|| std::env::var_os("VHS_STUDIO_SOURCE"))
        .map(std::path::PathBuf::from);
    // Dev hook: start exporting the loaded source to this path at launch, so
    // the in-app export (progress, cancel, completion) can be exercised
    // without a file dialog. The headless verifier covers the output itself.
    let initial_export = std::env::var_os("VHS_STUDIO_EXPORT").map(std::path::PathBuf::from);

    eframe::run_native(
        "VHS-Studio",
        options,
        Box::new(move |cc| {
            let mut app = vhs_studio_app::app::VhsStudioApp::new(cc);
            if let Some(path) = initial_source {
                app.load_source(path);
            }
            if let Some(path) = initial_export {
                app.export_from_ui(path);
            }
            Ok(Box::new(app))
        }),
    )
}
