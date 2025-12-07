use alvr_common::{lazy_static, prelude::*};
use wgpu::Adapter;

lazy_static! {
    static ref GPU_ADAPTERS: Vec<Adapter> = {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });

        instance.enumerate_adapters(wgpu::Backends::PRIMARY)
    };
}

pub enum GpuVendor {
    Nvidia,
    Amd,
    Other,
}

pub fn get_gpu_vendor() -> GpuVendor {
    match GPU_ADAPTERS[0].get_info().vendor {
        0x10de => GpuVendor::Nvidia,
        0x1002 => GpuVendor::Amd,
        _ => GpuVendor::Other,
    }
}

pub fn get_gpu_names() -> Vec<String> {
    GPU_ADAPTERS
        .iter()
        .map(|a| a.get_info().name)
        .collect::<Vec<_>>()
}

#[cfg(not(target_os = "macos"))]
pub fn get_screen_size() -> StrResult<(u32, u32)> {
    use std::sync::mpsc;
    use winit::{
        application::ApplicationHandler, event::WindowEvent, event_loop::EventLoop, window::Window,
    };

    #[cfg(target_os = "linux")]
    use winit::platform::wayland::EventLoopBuilderExtWayland;
    #[cfg(target_os = "windows")]
    use winit::platform::windows::EventLoopBuilderExtWindows;
    #[cfg(target_os = "linux")]
    use winit::platform::x11::EventLoopBuilderExtX11;

    struct ScreenSizeApp {
        tx: mpsc::Sender<Option<(u32, u32)>>,
    }

    impl ApplicationHandler for ScreenSizeApp {
        fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
            // Get primary monitor info
            if let Some(monitor) = event_loop.primary_monitor() {
                let size = monitor.size();
                let scale = monitor.scale_factor();
                let logical_width = (size.width as f64 / scale) as u32;
                let logical_height = (size.height as f64 / scale) as u32;
                let _ = self.tx.send(Some((logical_width, logical_height)));
            } else {
                // Fall back to creating a hidden window to get primary monitor
                let window_attrs = Window::default_attributes().with_visible(false);
                if let Ok(window) = event_loop.create_window(window_attrs) {
                    if let Some(monitor) = window.primary_monitor() {
                        let size = monitor.size();
                        let scale = monitor.scale_factor();
                        let logical_width = (size.width as f64 / scale) as u32;
                        let logical_height = (size.height as f64 / scale) as u32;
                        let _ = self.tx.send(Some((logical_width, logical_height)));
                    } else {
                        let _ = self.tx.send(None);
                    }
                } else {
                    let _ = self.tx.send(None);
                }
            }
            event_loop.exit();
        }

        fn window_event(
            &mut self,
            _event_loop: &winit::event_loop::ActiveEventLoop,
            _window_id: winit::window::WindowId,
            _event: WindowEvent,
        ) {
        }
    }

    let (tx, rx) = mpsc::channel();
    let mut app = ScreenSizeApp { tx };

    #[cfg(target_os = "windows")]
    let event_loop = trace_err!(EventLoop::builder().with_any_thread(true).build())?;
    #[cfg(target_os = "linux")]
    let event_loop = trace_err!({
        let mut builder = EventLoop::builder();
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            EventLoopBuilderExtWayland::with_any_thread(&mut builder, true);
        } else {
            EventLoopBuilderExtX11::with_any_thread(&mut builder, true);
        }
        builder.build()
    })?;
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    let event_loop = trace_err!(EventLoop::new())?;

    trace_err!(event_loop.run_app(&mut app))?;

    let result = trace_none!(rx.recv().ok().flatten())?;
    Ok(result)
}

#[cfg(target_os = "macos")]
pub fn get_screen_size() -> StrResult<(u32, u32)> {
    Ok((0, 0))
}
