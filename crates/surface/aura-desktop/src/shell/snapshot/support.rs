//! Drawing a screen and checking it, and reporting timings.

use crate::geom::rgba;
use crate::gpu::Offscreen;
use crate::shell::tests::Stage;
use crate::{raster, text};

/// Channel levels the GPU may be off the CPU rasterizer by: both blend
/// the same quads, and round differently.
const GPU_CLOSE: u8 = 1;

/// Draw the window, check something besides the background was drawn,
/// and save the screen when snapshots are asked for.
pub(super) fn snapshot(name: &str, stage: &mut Stage) {
    let scene = stage.shell.frame(stage.now).clone();
    let mut pixels = Vec::new();
    let on_gpu = std::env::var_os("AURA_DESKTOP_GPU").is_some();
    let drawn = text::with(|text| {
        raster::paint(&scene, text.atlas(), &mut pixels);
        on_gpu.then(|| {
            let mut gpu = Offscreen::new().expect("AURA_DESKTOP_GPU needs a graphics adapter");
            let close = GPU_CLOSE + u8::from(gpu.is_software());
            (gpu.read(&scene, text.atlas_mut()), close)
        })
    });
    assert_eq!(
        pixels.len(),
        scene.width as usize * scene.height as usize * 4
    );
    let bg = rgba(stage.shell.palette.bg);
    let inked = pixels.chunks(4).filter(|p| p[..3] != bg[..3]).count();
    assert!(inked > 10_000, "{name}: only {inked} pixels drawn");
    if let Some((drawn, close)) = drawn {
        assert_eq!(
            drawn.len(),
            pixels.len(),
            "{name}: the GPU drew another size"
        );
        let gaps = drawn.iter().zip(&pixels).map(|(a, b)| a.abs_diff(*b));
        let worst = gaps.max().unwrap_or(0);
        println!("{name}: the GPU is at most {worst} levels off the CPU");
        assert!(worst <= close, "{name}: the GPU is {worst} levels off");
    }
    if let Some(dir) = std::env::var_os("AURA_DESKTOP_SNAPSHOTS") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).expect("snapshot dir");
        let image = raster::png(scene.width, scene.height, &pixels);
        std::fs::write(dir.join(format!("{name}.png")), image).expect("write snapshot");
        std::fs::write(dir.join(format!("{name}.txt")), stage.shell.text()).expect("write text");
    }
}

pub fn report(label: &str, samples: &mut [std::time::Duration]) {
    samples.sort();
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
    let total: std::time::Duration = samples.iter().sum();
    let n = u32::try_from(samples.len()).unwrap_or(u32::MAX).max(1);
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    println!(
        "{label}: {} samples, mean {:.3} ms, p99 {:.3} ms, max {:.3} ms",
        samples.len(),
        ms(total / n),
        ms(p99),
        ms(samples[samples.len() - 1]),
    );
}
