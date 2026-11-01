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
    AgcConfig, AmplitudeParams, AmplitudeScratch, ClipStrategy, ColormapConfig, ColormapKind,
    ImageRenderParams, ImageRenderer, ImageViewport, NormScope, Result, TraceDataSource,
    process_trace_with_scratch,
};

/// Compact synthetic 100,000-trace × 2,000-sample source backed by a ring of 64 distinct
/// wavelet traces so benchmarks exercise realistic column-parallel decoding and min/max decimation
/// without allocating 800 MB of static bench fixture data.
struct SyntheticLargeSection {
    basis_traces: Vec<Vec<f32>>,
    trace_count: usize,
    sample_count: usize,
}

impl SyntheticLargeSection {
    fn new(trace_count: usize, sample_count: usize) -> Self {
        let basis_len = 64_usize;
        let mut basis_traces = Vec::with_capacity(basis_len);
        for t in 0..basis_len {
            let phase = (t as f32) * 0.15;
            let mut samples = Vec::with_capacity(sample_count);
            for s in 0..sample_count {
                let u = (s as f32) * 0.08 + phase;
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

impl TraceDataSource for SyntheticLargeSection {
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

fn bench_amplitude_pipeline(c: &mut Criterion) {
    let base_trace: Vec<f32> = (0..2000)
        .map(|i| ((i as f32) * 0.05).sin() * 50.0 + 1.5)
        .collect();
    let params_linear = AmplitudeParams::default()
        .with_scope(NormScope::Global)
        .with_clip(ClipStrategy::Rms { multiple: 2.0 })
        .with_gain_db(6.0);
    let params_agc = params_linear
        .with_dc_removal(true)
        .with_agc(AgcConfig::enabled(125));

    let mut work = base_trace.clone();
    let mut scratch = AmplitudeScratch::new();

    c.bench_function("amplitude_linear_2000_samples", |b| {
        b.iter(|| {
            work.copy_from_slice(&base_trace);
            process_trace_with_scratch(
                black_box(&mut work),
                black_box(&params_linear),
                Some(35.0),
                &mut scratch,
            );
        });
    });

    c.bench_function("amplitude_dc_agc_2000_samples", |b| {
        b.iter(|| {
            work.copy_from_slice(&base_trace);
            process_trace_with_scratch(
                black_box(&mut work),
                black_box(&params_agc),
                Some(2.0),
                &mut scratch,
            );
        });
    });
}

fn bench_image_renderer(c: &mut Criterion) {
    let section = SyntheticLargeSection::new(100_000, 2_000);
    let width = 1200_u32;
    let height = 800_u32;
    let mut out_rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    let mut renderer = ImageRenderer::new();

    let params = ImageRenderParams::default()
        .with_amplitude(
            AmplitudeParams::default()
                .with_scope(NormScope::Global)
                .with_clip(ClipStrategy::Rms { multiple: 2.0 }),
        )
        .with_colormap(ColormapConfig::new(ColormapKind::SeismicRwb))
        .with_max_traces_per_column(2)
        .with_global_reference(Some(50.0));

    let zoomed_out_vp = ImageViewport::new(width, height, 0..100_000, 0..2_000);
    c.bench_function("image_render_zoomed_out_100k_x_2000_to_1200x800", |b| {
        b.iter(|| {
            let res = renderer.render_into(
                black_box(&section),
                black_box(&zoomed_out_vp),
                black_box(&params),
                black_box(&mut out_rgba),
            );
            assert!(res.is_ok());
        });
    });

    let zoomed_in_vp = ImageViewport::new(width, height, 0..200, 0..400);
    c.bench_function("image_render_zoomed_in_200x400_to_1200x800", |b| {
        b.iter(|| {
            let res = renderer.render_into(
                black_box(&section),
                black_box(&zoomed_in_vp),
                black_box(&params),
                black_box(&mut out_rgba),
            );
            assert!(res.is_ok());
        });
    });
}

criterion_group!(benches, bench_amplitude_pipeline, bench_image_renderer);
criterion_main!(benches);
