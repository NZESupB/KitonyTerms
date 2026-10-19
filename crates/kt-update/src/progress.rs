//! 下载进度与速率的统一采样。
//!
//! 更新下载与 SFTP 下载共用同一实现：按固定时间窗口统计增量字节，得到瞬时
//! 速率，再用指数平滑压掉抖动，避免界面上的数字每帧乱跳。

use std::time::{Duration, Instant};

/// 采样窗口：窗口内的增量字节除以实际时长得到瞬时速率。
const SAMPLE_WINDOW: Duration = Duration::from_millis(250);
/// 指数平滑系数：新样本占比。越小越平稳、越迟钝。
const SMOOTHING: f64 = 0.3;

/// 一次下载的进度快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    /// 已接收字节数。
    pub received: u64,
    /// 平滑后的速率（字节/秒）；尚未完成一个采样窗口时为 0。
    pub speed: u64,
}

/// 速率采样器：按窗口计算并做指数平滑。
#[derive(Debug)]
pub struct RateMeter {
    window_start: Instant,
    window_bytes: u64,
    last_received: u64,
    speed: f64,
}

impl RateMeter {
    pub fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            window_bytes: 0,
            last_received: 0,
            speed: 0.0,
        }
    }

    /// 记录一次进度；到达采样窗口时更新速率。返回当前进度。
    ///
    /// 字节数倒退（例如重新开始的任务）按 0 增量处理，不会产生负速率。
    pub fn sample(&mut self, now: Instant, received: u64) -> Progress {
        let advance = received.saturating_sub(self.last_received);
        self.last_received = received;
        self.window_bytes = self.window_bytes.saturating_add(advance);
        let elapsed = now.saturating_duration_since(self.window_start);
        if elapsed >= SAMPLE_WINDOW {
            let instant = self.window_bytes as f64 / elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
            self.speed = if self.speed == 0.0 {
                // 首个窗口直接采用瞬时值，避免从 0 缓慢爬升。
                instant
            } else {
                self.speed * (1.0 - SMOOTHING) + instant * SMOOTHING
            };
            self.window_start = now;
            self.window_bytes = 0;
        }
        Progress {
            received,
            speed: self.speed.max(0.0) as u64,
        }
    }

    /// 结束采样：速率归零，避免完成后仍显示最后一次速度。
    pub fn finish(&mut self, received: u64) -> Progress {
        self.last_received = received;
        self.window_bytes = 0;
        self.speed = 0.0;
        Progress { received, speed: 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_window_uses_instant_rate() {
        let start = Instant::now();
        let mut meter = RateMeter::new(start);
        assert_eq!(meter.sample(start, 0).speed, 0);
        // 未到窗口边界：速率保持 0，不虚报。
        assert_eq!(
            meter.sample(start + Duration::from_millis(100), 500).speed,
            0
        );
        let progress = meter.sample(start + Duration::from_millis(500), 5_000);
        assert_eq!(progress.received, 5_000);
        // 500ms 内 5000 字节 → 10000 B/s。
        assert_eq!(progress.speed, 10_000);
    }

    #[test]
    fn later_windows_are_smoothed() {
        let start = Instant::now();
        let mut meter = RateMeter::new(start);
        assert_eq!(meter.sample(start, 0).speed, 0);
        assert_eq!(
            meter
                .sample(start + Duration::from_millis(250), 2_500)
                .speed,
            10_000
        );
        // 与首窗口同速时保持读数。
        assert_eq!(
            meter
                .sample(start + Duration::from_millis(500), 5_000)
                .speed,
            10_000
        );
        // 瞬时速率降到 2000 B/s：平滑后约为 7600，而不是直接跳到瞬时值。
        let dropped = meter
            .sample(start + Duration::from_millis(750), 5_500)
            .speed;
        assert!(
            (7_500..=7_700).contains(&dropped),
            "平滑后应介于两者之间: {dropped}"
        );
    }

    #[test]
    fn backwards_progress_never_yields_negative_speed() {
        let start = Instant::now();
        let mut meter = RateMeter::new(start);
        let peak = meter
            .sample(start + Duration::from_millis(250), 1_000)
            .speed;
        assert_eq!(peak, 4_000);
        // 字节数倒退按 0 增量处理：读数只会被平滑拉低，不会变成负值或跳变。
        let progress = meter.sample(start + Duration::from_millis(500), 10);
        assert_eq!(progress.received, 10);
        assert!(progress.speed < peak, "读数应被拉低: {}", progress.speed);
    }

    #[test]
    fn finish_zeroes_the_rate() {
        let start = Instant::now();
        let mut meter = RateMeter::new(start);
        assert_ne!(
            meter
                .sample(start + Duration::from_millis(250), 2_500)
                .speed,
            0
        );
        let finished = meter.finish(2_500);
        assert_eq!(finished.received, 2_500);
        assert_eq!(finished.speed, 0);
    }
}
