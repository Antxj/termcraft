//! "Remote monitoring": resource usage of an SSH server, shown in a bar
//! below its terminal. One long-running exec channel runs
//! [`SCRIPT`] (POSIX sh, Linux `/proc`), which prints a block of lines every
//! two seconds; [`MonitorParser`] turns blocks into [`Sample`]s.

use std::time::Instant;

/// Sampling script. Prints `cpu`, `mem`, `net`, `disk`, `up` and `users`
/// lines, then `end`, every 2 s. Unknown systems print nothing useful and
/// the bar says so.
pub const SCRIPT: &str = r#"while :; do
read -r _ u n s i w x y z _ 2>/dev/null < /proc/stat && echo "cpu $u $n $s $i $w $x $y $z"
awk '/^MemTotal:/{t=$2}/^MemAvailable:/{a=$2}END{print "mem",t+0,a+0}' /proc/meminfo 2>/dev/null
awk 'NR>2{sub(/:/," ");if($1!="lo"){rx+=$2;tx+=$10}}END{print "net",rx+0,tx+0}' /proc/net/dev 2>/dev/null
df -Pk / 2>/dev/null | awk 'NR==2{print "disk",$2+0,$3+0}'
echo "up $(cut -d' ' -f1 /proc/uptime 2>/dev/null)"
echo "users $(who 2>/dev/null | wc -l)"
echo end
sleep 2
done"#;

/// One reading. Rates need two readings, so the first sample has none.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    pub cpu_percent: Option<f32>,
    /// KiB.
    pub mem_total: Option<u64>,
    pub mem_used: Option<u64>,
    /// Bytes per second, all interfaces but `lo`.
    pub rx_rate: Option<f64>,
    pub tx_rate: Option<f64>,
    /// KiB, root file system.
    pub disk_total: Option<u64>,
    pub disk_used: Option<u64>,
    pub uptime_secs: Option<u64>,
    pub users: Option<u32>,
}

#[derive(Default)]
pub struct MonitorParser {
    line: Vec<u8>,
    current: Sample,
    cpu: Option<(u64, u64)>,
    prev_cpu: Option<(u64, u64)>,
    net: Option<(u64, u64)>,
    prev_net: Option<(u64, u64, Instant)>,
}

/// Longest line kept; anything longer is garbage and dropped.
const MAX_LINE: usize = 512;

fn nums<T: std::str::FromStr>(rest: &str) -> Vec<T> {
    rest.split_whitespace()
        .filter_map(|w| w.parse().ok())
        .collect()
}

impl MonitorParser {
    /// Feed channel output; returns the samples completed by it.
    pub fn feed(&mut self, bytes: &[u8], now: Instant) -> Vec<Sample> {
        let mut out = Vec::new();
        for &b in bytes {
            if b == b'\n' {
                let line = String::from_utf8_lossy(&self.line).into_owned();
                self.line.clear();
                if let Some(sample) = self.line_done(line.trim(), now) {
                    out.push(sample);
                }
            } else if self.line.len() < MAX_LINE {
                self.line.push(b);
            }
        }
        out
    }

    fn line_done(&mut self, line: &str, now: Instant) -> Option<Sample> {
        let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
        let s = &mut self.current;
        match key {
            "cpu" => {
                let v: Vec<u64> = nums(rest);
                if v.len() >= 5 {
                    let total = v.iter().sum();
                    let idle = v[3] + v[4];
                    self.cpu = Some((total, idle));
                }
            }
            "mem" => {
                if let [total, available] = nums::<u64>(rest)[..]
                    && total > 0
                {
                    s.mem_total = Some(total);
                    s.mem_used = Some(total.saturating_sub(available));
                }
            }
            "net" => {
                if let [rx, tx] = nums::<u64>(rest)[..] {
                    self.net = Some((rx, tx));
                }
            }
            "disk" => {
                if let [total, used] = nums::<u64>(rest)[..]
                    && total > 0
                {
                    s.disk_total = Some(total);
                    s.disk_used = Some(used);
                }
            }
            "up" => {
                s.uptime_secs = rest.split('.').next().and_then(|v| v.trim().parse().ok());
            }
            "users" => s.users = rest.trim().parse().ok(),
            "end" => return Some(self.finish(now)),
            _ => {}
        }
        None
    }

    fn finish(&mut self, now: Instant) -> Sample {
        let mut sample = std::mem::take(&mut self.current);
        if let (Some((total, idle)), Some((pt, pi))) = (self.cpu, self.prev_cpu) {
            let dt = total.saturating_sub(pt);
            if dt > 0 {
                let busy = dt.saturating_sub(idle.saturating_sub(pi));
                sample.cpu_percent = Some((busy as f32 * 100.0 / dt as f32).clamp(0.0, 100.0));
            }
        }
        if let Some(cpu) = self.cpu.take() {
            self.prev_cpu = Some(cpu);
        }
        if let Some((rx, tx)) = self.net.take() {
            if let Some((prx, ptx, at)) = self.prev_net {
                let secs = now.duration_since(at).as_secs_f64();
                if secs > 0.0 {
                    sample.rx_rate = Some(rx.saturating_sub(prx) as f64 / secs);
                    sample.tx_rate = Some(tx.saturating_sub(ptx) as f64 / secs);
                }
            }
            self.prev_net = Some((rx, tx, now));
        }
        sample
    }
}

/// `1.5 MB/s` style rate.
pub fn human_rate(bytes_per_sec: f64) -> String {
    format!("{}/s", human_bytes(bytes_per_sec))
}

pub fn human_bytes(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes.max(0.0);
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{v:.0} {}", UNITS[unit])
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// `3d 4h`, `5h 12m`, `7m`.
pub fn human_uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const BLOCK1: &str = "cpu 100 0 100 700 100 0 0 0\nmem 4000000 1000000\nnet 1000 2000\n\
                          disk 10000000 2500000\nup 93784.12\nusers 2\nend\n";
    const BLOCK2: &str = "cpu 150 0 150 750 150 0 0 0\nmem 4000000 2000000\nnet 3048 6096\n\
                          disk 10000000 2500000\nup 93786.10\nusers 2\nend\n";

    #[test]
    fn parses_blocks_and_computes_rates() {
        let t0 = Instant::now();
        let mut p = MonitorParser::default();
        let first = p.feed(BLOCK1.as_bytes(), t0);
        assert_eq!(first.len(), 1);
        let s = &first[0];
        assert_eq!(s.cpu_percent, None, "needs two readings");
        assert_eq!(s.mem_used, Some(3_000_000));
        assert_eq!(s.disk_used, Some(2_500_000));
        assert_eq!(s.uptime_secs, Some(93_784));
        assert_eq!(s.users, Some(2));

        // Split across reads, 2 s later.
        let (a, b) = BLOCK2.split_at(17);
        assert!(p.feed(a.as_bytes(), t0).is_empty());
        let second = p.feed(b.as_bytes(), t0 + Duration::from_secs(2));
        let s = &second[0];
        // total +200, idle (idle+iowait) +100 -> 50 % busy.
        assert_eq!(s.cpu_percent, Some(50.0));
        assert_eq!(s.rx_rate, Some(1024.0));
        assert_eq!(s.tx_rate, Some(2048.0));
        assert_eq!(s.mem_used, Some(2_000_000));
    }

    #[test]
    fn garbage_is_ignored() {
        let mut p = MonitorParser::default();
        let long = "x".repeat(10_000);
        let out = p.feed(
            format!("{long}\nmem a b\nnope\nend\n").as_bytes(),
            Instant::now(),
        );
        assert_eq!(out, vec![Sample::default()]);
    }

    #[test]
    fn human_formats() {
        assert_eq!(human_bytes(512.0), "512 B");
        assert_eq!(human_rate(1536.0), "1.5 KB/s");
        assert_eq!(human_uptime(93_784), "1d 2h");
        assert_eq!(human_uptime(3_720), "1h 2m");
        assert_eq!(human_uptime(59), "0m");
    }
}
