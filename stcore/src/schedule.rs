//! 定时更新调度（子项 3 所有权）：间隔 / 定点 / 从不 三态
//!
//! 实现选择：std::thread + 可中断 sleep（不引 tokio，保持 app 依赖面——见 §8.4 子项 3 备注）。
//! 线程只在 mode != Never 且 roots 非空时存在；stop() 后 1s 内退出。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::{ScheduleConfig, ScheduleMode};

/// 中断轮询粒度：sleep 分片，保证 stop 及时
const POLL_SLICE: Duration = Duration::from_millis(200);

/// 计算"从现在起到下次触发"的等待时长；`None` = 不触发（Never）
pub fn next_delay(cfg: &ScheduleConfig, now: SystemTime) -> Option<Duration> {
    match cfg.mode {
        ScheduleMode::Never => None,
        ScheduleMode::Interval => {
            let mins = cfg.interval_minutes.max(1);
            Some(Duration::from_secs(mins * 60))
        }
        ScheduleMode::Daily => {
            let (h, m) = parse_hhmm(&cfg.daily_at).unwrap_or((3, 0));
            let secs_of_day = now.duration_since(UNIX_EPOCH).ok()?.as_secs() % 86_400;
            let target = h as u64 * 3600 + m as u64 * 60;
            let delta = if target > secs_of_day {
                target - secs_of_day
            } else {
                86_400 - secs_of_day + target
            };
            Some(Duration::from_secs(delta.max(1)))
        }
    }
}

/// "HH:mm" → (时, 分)；非法输入返回 None
pub fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    (h < 24 && m < 60).then_some((h, m))
}

/// 定时索引器句柄
pub struct Scheduler {
    stop_flag: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    /// 已完成触发次数（可观测/测试用）
    ticks: Arc<std::sync::atomic::AtomicU64>,
}

impl Scheduler {
    /// 启动调度线程。
    ///
    /// `job` 在每次触发时调用（应由调用方在内部做"索引 roots"的重活）。
    /// mode=Never 或 roots 为空时不启动线程，返回空句柄。
    pub fn start(
        cfg: ScheduleConfig,
        roots: Vec<PathBuf>,
        job: impl Fn(&[PathBuf]) + Send + 'static,
    ) -> Self {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let ticks = Arc::new(std::sync::atomic::AtomicU64::new(0));

        if cfg.mode == ScheduleMode::Never || roots.is_empty() {
            return Self {
                stop_flag,
                handle: None,
                ticks,
            };
        }

        let sf = stop_flag.clone();
        let tk = ticks.clone();
        let handle = thread::spawn(move || {
            // 每次循环重新计算 delay（daily 会跨天）
            while !sf.load(Ordering::Relaxed) {
                let Some(delay) = next_delay(&cfg, SystemTime::now()) else {
                    break;
                };
                if !interruptible_sleep(&sf, delay) {
                    break; // 被 stop 唤醒
                }
                if sf.load(Ordering::Relaxed) {
                    break;
                }
                job(&roots);
                tk.fetch_add(1, Ordering::Relaxed);
            }
        });

        Self {
            stop_flag,
            handle: Some(handle),
            ticks,
        }
    }

    pub fn is_running(&self) -> bool {
        self.handle.is_some() && !self.stop_flag.load(Ordering::Relaxed)
    }

    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }

    /// 停止并回收线程（join 阻塞至多一个 POLL_SLICE）
    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 可中断 sleep：返回 false 表示期间收到停止信号
fn interruptible_sleep(stop: &AtomicBool, total: Duration) -> bool {
    let mut left = total;
    while left > Duration::ZERO {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let step = left.min(POLL_SLICE);
        thread::sleep(step);
        left -= step;
    }
    !stop.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hhmm_validates() {
        assert_eq!(parse_hhmm("03:00"), Some((3, 0)));
        assert_eq!(parse_hhmm("23:59"), Some((23, 59)));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);
        assert_eq!(parse_hhmm("abc"), None);
    }

    #[test]
    fn never_mode_has_no_delay() {
        let cfg = ScheduleConfig {
            mode: ScheduleMode::Never,
            ..Default::default()
        };
        assert!(next_delay(&cfg, SystemTime::now()).is_none());
    }

    #[test]
    fn interval_delay_is_config_minutes() {
        let cfg = ScheduleConfig {
            mode: ScheduleMode::Interval,
            interval_minutes: 30,
            daily_at: "00:00".into(),
        };
        assert_eq!(
            next_delay(&cfg, SystemTime::now()),
            Some(Duration::from_secs(30 * 60))
        );
    }

    #[test]
    fn interval_minimum_one_minute() {
        let cfg = ScheduleConfig {
            mode: ScheduleMode::Interval,
            interval_minutes: 0,
            daily_at: "00:00".into(),
        };
        assert_eq!(
            next_delay(&cfg, SystemTime::now()),
            Some(Duration::from_secs(60))
        );
    }

    #[test]
    fn daily_delay_within_24h_and_positive() {
        let cfg = ScheduleConfig {
            mode: ScheduleMode::Daily,
            interval_minutes: 60,
            daily_at: "03:00".into(),
        };
        let d = next_delay(&cfg, SystemTime::now()).unwrap();
        assert!(d > Duration::ZERO);
        assert!(d <= Duration::from_secs(86_400));
    }

    #[test]
    fn scheduler_never_mode_does_not_spawn() {
        let cfg = ScheduleConfig {
            mode: ScheduleMode::Never,
            ..Default::default()
        };
        let s = Scheduler::start(cfg, vec![PathBuf::from("/tmp")], |_| {});
        assert!(!s.is_running());
        assert_eq!(s.ticks(), 0);
    }

    #[test]
    fn scheduler_empty_roots_does_not_spawn() {
        let cfg = ScheduleConfig {
            mode: ScheduleMode::Interval,
            interval_minutes: 1,
            daily_at: "00:00".into(),
        };
        let s = Scheduler::start(cfg, vec![], |_| {});
        assert!(!s.is_running());
    }

    #[test]
    fn scheduler_can_be_stopped() {
        let cfg = ScheduleConfig {
            mode: ScheduleMode::Interval,
            interval_minutes: 60,
            daily_at: "00:00".into(),
        };
        let mut s = Scheduler::start(cfg, vec![PathBuf::from("/tmp")], |_| {});
        assert!(s.is_running());
        s.stop();
        assert!(!s.is_running());
    }

    #[test]
    fn stop_is_idempotent() {
        let cfg = ScheduleConfig {
            mode: ScheduleMode::Interval,
            interval_minutes: 60,
            daily_at: "00:00".into(),
        };
        let mut s = Scheduler::start(cfg, vec![PathBuf::from("/tmp")], |_| {});
        s.stop();
        s.stop(); // 二次 stop 不应 panic
        assert!(!s.is_running());
        assert_eq!(s.ticks(), 0);
    }

    #[test]
    fn daily_delay_is_exact_when_now_known() {
        // 目标时刻 03:00 → target = 10800s
        let cfg = ScheduleConfig {
            mode: ScheduleMode::Daily,
            interval_minutes: 60,
            daily_at: "03:00".into(),
        };
        // now = 02:00 当天 → 距 03:00 还有 3600s
        let at_0200 = UNIX_EPOCH + Duration::from_secs(2 * 3600);
        assert_eq!(next_delay(&cfg, at_0200), Some(Duration::from_secs(3600)));
        // now = 04:00 当天 → 已过点 → 顺延到次日 03:00 = 82800s
        let at_0400 = UNIX_EPOCH + Duration::from_secs(4 * 3600);
        assert_eq!(next_delay(&cfg, at_0400), Some(Duration::from_secs(82_800)));
        // 恰在触发点 → 顺延整整一天
        let at_0300 = UNIX_EPOCH + Duration::from_secs(10_800);
        assert_eq!(next_delay(&cfg, at_0300), Some(Duration::from_secs(86_400)));
    }

    #[test]
    fn daily_invalid_hhmm_falls_back_to_0300() {
        let cfg = ScheduleConfig {
            mode: ScheduleMode::Daily,
            interval_minutes: 60,
            daily_at: "25:99".into(), // 非法 → 回退 03:00
        };
        let at_0200 = UNIX_EPOCH + Duration::from_secs(2 * 3600);
        assert_eq!(next_delay(&cfg, at_0200), Some(Duration::from_secs(3600)));
    }

    #[test]
    fn scheduler_interval_with_roots_is_running() {
        let cfg = ScheduleConfig {
            mode: ScheduleMode::Interval,
            interval_minutes: 5,
            daily_at: "00:00".into(),
        };
        let s = Scheduler::start(cfg, vec![PathBuf::from("/tmp")], |_| {});
        assert!(s.is_running());
    }
}
