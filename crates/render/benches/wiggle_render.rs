#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::hint::black_box;
use std::ops::Range;

use criterion::{Criterion, criterion_group, criterion_main};
use render::{
    AmplitudeParams, ClipStrategy, ImageViewport, NormScope, Result, TraceDataSource,
    WiggleFillMode, WiggleRenderParams, WiggleRenderer, render_wiggle_geometry,
};

struct SyntheticWiggleSection {
    basis_traces: Vec<Vec<f32>>,
    trace_count: usize,
    sample_count: usize,
}

impl SyntheticWiggleSection {
    fn new(trace_count: usize, sample_count: usize) -> Self {
        let basis_len = 64_usize;
        let mut basis_traces = Vec::with_capacity(basis_len);
        for t in 0..basis_len {
            let phase = (t as f32) * 0.2;
            let mut samples = Vec::with_capacity(sample_count);
            for s in 0..sample_count {
                let u = (s as f32) * 0.09 + phase;
                let wavelet = u.sin() * (-(s as f32 - 1000.0).powi(2) / 250_000.0).exp();
                samples.push(wavelet * 100.0);
            }
            basis_traces.push(samples);
        }
        Self {
            basis_traces,
            trace_count,
            sample_count,
        }
    }
}

impl TraceDataSource for SyntheticWiggleSection {
    #[inline]
    fn trace_count(&self) -> usize {
        self.trace_count
    }

    #[inline]
    fn sample_count(&self) -> usize {
        self.sample_count
    }

    #[inline]
    fn read_trace_window(
        &self,
        trace_index: usize,
        sample_range: Range<usize>,
        out: &mut Vec<f32>,
    ) -> Result<()> {
        let basis = &self.basis_traces[trace_index % self.basis_traces.len()];
        let end = sample_range.end.min(basis.len());
        let start = sample_range.start.min(end);
        out.clear();
        out.extend_from_slice(&basis[start..end]);
        Ok(())
    }
}

fn bench_wiggle_renderer(c: &mut Criterion) {
    let section = SyntheticWiggleSection::new(100_000, 2_000);
    let width = 1200_u32;
    let height = 800_u32;
    let viewport = ImageViewport::new(width, height, 0..120, 0..2_000);
    let params = WiggleRenderParams::default()
        .with_amplitude(
            AmplitudeParams::default()
                .with_scope(NormScope::Global)
                .with_clip(ClipStrategy::Rms { multiple: 2.0 }),
        )
        .with_fill_mode(WiggleFillMode::Positive)
        .with_excursion_spacings(1.2)
        .with_clip_spacings(2.0)
        .with_global_reference(Some(50.0));

    c.bench_function("wiggle_geometry_120x2000_decimated_to_1200x800", |b| {
        b.iter(|| {
            let geom = render_wiggle_geometry(
                black_box(&section),
                black_box(&viewport),
                black_box(&params),
            );
            assert!(geom.is_ok());
        });
    });

    let mut renderer = WiggleRenderer::new();
    let mut out_rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    c.bench_function("wiggle_rasterize_120x2000_to_1200x800_rgba", |b| {
        b.iter(|| {
            let res = renderer.render_into(
                black_box(&section),
                black_box(&viewport),
                black_box(&params),
                true,
                black_box(&mut out_rgba),
            );
            assert!(res.is_ok());
        });
    });
}

criterion_group!(benches, bench_wiggle_renderer);
criterion_main!(benches);
