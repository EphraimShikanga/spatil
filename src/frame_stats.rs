use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const WINDOW: usize = 1200;
const LOG_EVERY: Duration = Duration::from_secs(5);
const LATE_FACTOR: f32 = 1.5;

#[derive(Default)]
struct Series(VecDeque<f32>);

impl Series {
    fn push(&mut self, ms: f32) {
        if self.0.len() == WINDOW {
            self.0.pop_front();
        }
        self.0.push_back(ms);
    }

    fn summary(&self) -> Value {
        if self.0.is_empty() {
            return Value::Null;
        }
        let mut sorted: Vec<f32> = self.0.iter().copied().collect();
        sorted.sort_by(f32::total_cmp);
        let last = sorted.len() - 1;
        let at = |p: f32| round(sorted[(last as f32 * p).round() as usize]);
        let avg = sorted.iter().sum::<f32>() / sorted.len() as f32;
        json!({
            "n": sorted.len(),
            "avg": round(avg),
            "p50": at(0.50),
            "p95": at(0.95),
            "p99": at(0.99),
            "max": round(sorted[last]),
        })
    }
}

fn round(ms: f32) -> f64 {
    (f64::from(ms) * 1000.0).round() / 1000.0
}

fn ms(d: Duration) -> f32 {
    d.as_secs_f32() * 1000.0
}

pub struct FrameSample {
    pub tick: Duration,
    pub render_cpu: Duration,
    pub gpu_wait: Option<Duration>,
    pub total: Duration,
}

pub struct FrameStats {
    tick: Series,
    render_cpu: Series,
    gpu_wait: Series,
    frame: Series,
    interval: Series,
    shm_upload: Series,
    frames: u64,
    skipped: u64,
    late: u64,
    missed_refreshes: u64,
    shm_commits: u64,
    dmabuf_commits: u64,
    refresh_hz: Option<u32>,
    since: Instant,
    log: bool,
    last_log: Instant,
}

impl FrameStats {
    pub fn new(log: bool) -> Self {
        let now = Instant::now();
        Self {
            tick: Series::default(),
            render_cpu: Series::default(),
            gpu_wait: Series::default(),
            frame: Series::default(),
            interval: Series::default(),
            shm_upload: Series::default(),
            frames: 0,
            skipped: 0,
            late: 0,
            missed_refreshes: 0,
            shm_commits: 0,
            dmabuf_commits: 0,
            refresh_hz: None,
            since: now,
            log,
            last_log: now,
        }
    }

    pub fn set_refresh_hz(&mut self, hz: u32) {
        self.refresh_hz = (hz > 0).then_some(hz);
    }

    pub fn record_frame(&mut self, sample: FrameSample) {
        self.frames += 1;
        self.tick.push(ms(sample.tick));
        self.render_cpu.push(ms(sample.render_cpu));
        if let Some(wait) = sample.gpu_wait {
            self.gpu_wait.push(ms(wait));
        }
        self.frame.push(ms(sample.total));
        self.maybe_log();
    }

    pub fn record_interval(&mut self, interval_ms: f32) {
        if interval_ms <= 0.0 {
            return;
        }
        self.interval.push(interval_ms);
        if let Some(hz) = self.refresh_hz {
            let period = 1000.0 / hz as f32;
            if interval_ms > period * LATE_FACTOR {
                self.late += 1;
                self.missed_refreshes += ((interval_ms / period).round() as u64).saturating_sub(1);
            }
        }
    }

    pub fn record_skipped(&mut self) {
        self.skipped += 1;
    }

    pub fn record_shm_upload(&mut self, took: Duration) {
        self.shm_commits += 1;
        self.shm_upload.push(ms(took));
    }

    pub fn record_dmabuf_commit(&mut self) {
        self.dmabuf_commits += 1;
    }

    pub fn reset(&mut self) {
        let (refresh_hz, log) = (self.refresh_hz, self.log);
        *self = Self::new(log);
        self.refresh_hz = refresh_hz;
    }

    pub fn report(&self, backend: &str, dmabuf_advertised: bool) -> Value {
        json!({
            "backend": backend,
            "dmabuf_advertised": dmabuf_advertised,
            "refresh_hz": self.refresh_hz,
            "seconds": round(self.since.elapsed().as_secs_f32()),
            "window": WINDOW,
            "frames": self.frames,
            "skipped_frames": self.skipped,
            "late_frames": self.late,
            "missed_refreshes": self.missed_refreshes,
            "shm_commits": self.shm_commits,
            "dmabuf_commits": self.dmabuf_commits,
            "tick_ms": self.tick.summary(),
            "render_cpu_ms": self.render_cpu.summary(),
            "gpu_wait_ms": self.gpu_wait.summary(),
            "frame_ms": self.frame.summary(),
            "vblank_interval_ms": self.interval.summary(),
            "shm_upload_ms": self.shm_upload.summary(),
        })
    }

    fn maybe_log(&mut self) {
        if !self.log || self.last_log.elapsed() < LOG_EVERY {
            return;
        }
        self.last_log = Instant::now();
        let (frame, wait, interval, upload) = (
            self.frame.summary(),
            self.gpu_wait.summary(),
            self.interval.summary(),
            self.shm_upload.summary(),
        );
        tracing::info!(
            target: "spatil::frame_stats",
            "{} frames | frame p50 {} p95 {} max {} | gpu wait p95 {} | interval p95 {} | \
             late {} missed {} skipped {} | shm uploads {} (p95 {}) dmabuf commits {}",
            self.frames,
            frame["p50"],
            frame["p95"],
            frame["max"],
            wait["p95"],
            interval["p95"],
            self.late,
            self.missed_refreshes,
            self.skipped,
            self.shm_commits,
            upload["p95"],
            self.dmabuf_commits,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_reports_percentiles() {
        let mut s = Series::default();
        for v in 1..=100 {
            s.push(v as f32);
        }
        let r = s.summary();
        assert_eq!(r["n"], 100);
        assert_eq!(r["p50"], 51.0);
        assert_eq!(r["p95"], 95.0);
        assert_eq!(r["max"], 100.0);
        assert_eq!(r["avg"], 50.5);
    }

    #[test]
    fn empty_series_is_null() {
        assert!(Series::default().summary().is_null());
    }

    #[test]
    fn series_keeps_only_the_window() {
        let mut s = Series::default();
        for v in 0..(WINDOW + 10) {
            s.push(v as f32);
        }
        assert_eq!(s.0.len(), WINDOW);
        assert_eq!(s.0.front().copied(), Some(10.0));
    }

    #[test]
    fn late_intervals_count_missed_refreshes() {
        let mut stats = FrameStats::new(false);
        stats.set_refresh_hz(60);
        stats.record_interval(16.7);
        stats.record_interval(33.3);
        stats.record_interval(50.0);
        assert_eq!(stats.late, 2);
        assert_eq!(stats.missed_refreshes, 3);
    }

    #[test]
    fn no_refresh_rate_means_no_late_counting() {
        let mut stats = FrameStats::new(false);
        stats.record_interval(100.0);
        assert_eq!(stats.late, 0);
        assert_eq!(stats.interval.0.len(), 1);
    }

    #[test]
    fn reset_clears_samples_but_keeps_refresh_rate() {
        let mut stats = FrameStats::new(false);
        stats.set_refresh_hz(144);
        stats.record_shm_upload(Duration::from_millis(2));
        stats.record_dmabuf_commit();
        stats.reset();
        let r = stats.report("drm", true);
        assert_eq!(r["shm_commits"], 0);
        assert_eq!(r["dmabuf_commits"], 0);
        assert!(r["shm_upload_ms"].is_null());
        assert_eq!(r["refresh_hz"], 144);
    }
}
