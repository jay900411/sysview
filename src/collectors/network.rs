//! Network collector：`/proc/net/dev`、`/sys/class/net`、`getifaddrs(3)`、`/proc/net/sockstat`。
//!
//! IP 位址用 `getifaddrs(3)` 取得而不是 fork 一個 `ip` 行程 ——
//! Availability 要求不能每隔幾秒就 fork 一次外部指令。

use std::time::Instant;

use crate::collectors::util::{self, Counter};
use crate::error::Unavailable;
use crate::metrics::model::{Reading, Series, Unit};

/// `/proc/net/dev` 一行解析出來的累計計數。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IfCounters {
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub rx_errs: u64,
    pub rx_drop: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
    pub tx_errs: u64,
    pub tx_drop: u64,
}

/// 解析 `/proc/net/dev` 的一行。
///
/// 格式是 `  eth0: 1234 5 0 0 ...`，介面名與數字之間**沒有保證有空白**
/// （流量大時會變成 `eth0:12345678`），所以必須以第一個 `:` 切開。
pub fn parse_net_dev_line(line: &str) -> Option<(String, IfCounters)> {
    let (name, rest) = line.split_once(':')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let f: Vec<u64> = rest
        .split_ascii_whitespace()
        .map(|v| v.parse::<u64>().unwrap_or(0))
        .collect();
    if f.len() < 16 {
        return None;
    }
    Some((
        name.to_owned(),
        IfCounters {
            rx_bytes: f[0],
            rx_packets: f[1],
            rx_errs: f[2],
            rx_drop: f[3],
            tx_bytes: f[8],
            tx_packets: f[9],
            tx_errs: f[10],
            tx_drop: f[11],
        },
    ))
}

/// 解析整份 `/proc/net/dev`（跳過兩行表頭）。
pub fn parse_net_dev(text: &str) -> Vec<(String, IfCounters)> {
    text.lines()
        .skip(2)
        .filter_map(parse_net_dev_line)
        .collect()
}

/// 這個介面該不該算進「系統總流量」。
/// lo 的流量會同時算在 rx 和 tx，計入會讓總量虛胖一倍。
pub fn is_virtual_interface(name: &str) -> bool {
    name == "lo"
        || name.starts_with("veth")
        || name.starts_with("br-")
        || name.starts_with("virbr")
        || name.starts_with("docker")
        || name.starts_with("tun")
        || name.starts_with("tap")
}

#[derive(Debug, Clone)]
pub struct Interface {
    pub name: String,
    pub up: bool,
    pub virtual_iface: bool,
    pub mac: Option<String>,
    pub speed_mbit: Option<i64>,
    pub addrs: Vec<String>,
    pub rx_rate: Reading,
    pub tx_rate: Reading,
    pub rx_total: u64,
    pub tx_total: u64,
    pub errors: u64,
    pub drops: u64,
    pub rx_history: Series,
    pub tx_history: Series,
    rx_ctr: Counter,
    tx_ctr: Counter,
}

impl Interface {
    fn new(name: String, history_len: usize) -> Self {
        Self {
            virtual_iface: is_virtual_interface(&name),
            name,
            up: false,
            mac: None,
            speed_mbit: None,
            addrs: Vec::new(),
            rx_rate: Reading::unavailable(Unit::BytesPerSec, Unavailable::Pending),
            tx_rate: Reading::unavailable(Unit::BytesPerSec, Unavailable::Pending),
            rx_total: 0,
            tx_total: 0,
            errors: 0,
            drops: 0,
            rx_history: Series::new(history_len),
            tx_history: Series::new(history_len),
            rx_ctr: Counter::new(),
            tx_ctr: Counter::new(),
        }
    }
}

/// `/proc/net/sockstat` 摘要。
#[derive(Debug, Clone, Default)]
pub struct SocketStats {
    pub tcp_inuse: Option<u64>,
    pub tcp_tw: Option<u64>,
    pub tcp_orphan: Option<u64>,
    pub udp_inuse: Option<u64>,
    pub sockets_used: Option<u64>,
    pub raw_lines: Vec<String>,
}

pub fn parse_sockstat(text: &str) -> SocketStats {
    let mut s = SocketStats::default();
    for line in text.lines() {
        s.raw_lines.push(line.to_owned());
        let Some((proto, rest)) = line.split_once(':') else {
            continue;
        };
        let kv: Vec<&str> = rest.split_ascii_whitespace().collect();
        let get = |key: &str| -> Option<u64> {
            kv.windows(2)
                .find(|w| w[0] == key)
                .and_then(|w| w[1].parse().ok())
        };
        match proto {
            "sockets" => s.sockets_used = get("used"),
            "TCP" => {
                s.tcp_inuse = get("inuse");
                s.tcp_tw = get("tw");
                s.tcp_orphan = get("orphan");
            }
            "UDP" => s.udp_inuse = get("inuse"),
            _ => {}
        }
    }
    s
}

#[derive(Default)]
pub struct NetState {
    pub interfaces: Vec<Interface>,
    pub total_rx: Reading,
    pub total_tx: Reading,
    pub rx_history: Series,
    pub tx_history: Series,
    pub sockets: SocketStats,
}

pub struct NetCollector {
    state: NetState,
    index: std::collections::HashMap<String, usize>,
    buf: String,
    history_len: usize,
    last_addr_scan: Option<Instant>,
}

impl NetCollector {
    pub fn new(history_len: usize) -> Self {
        Self {
            state: NetState {
                rx_history: Series::new(history_len),
                tx_history: Series::new(history_len),
                ..Default::default()
            },
            index: std::collections::HashMap::new(),
            buf: String::with_capacity(8192),
            history_len,
            last_addr_scan: None,
        }
    }
    pub fn state(&self) -> &NetState {
        &self.state
    }

    pub fn update(&mut self, now: Instant) {
        if util::read_into("/proc/net/dev", &mut self.buf).is_ok() {
            let text = std::mem::take(&mut self.buf);
            self.update_counters(&text, now);
            self.buf = text;
        }
        // IP 位址與連線速度變動很慢，10 秒查一次就夠。
        let need = self
            .last_addr_scan
            .is_none_or(|t| now.duration_since(t).as_secs_f64() >= 10.0);
        if need {
            self.read_addrs();
            self.read_link_info();
            self.last_addr_scan = Some(now);
        }
        if let Ok(t) = util::read_string("/proc/net/sockstat") {
            self.state.sockets = parse_sockstat(&t);
        }
    }

    fn update_counters(&mut self, text: &str, now: Instant) {
        let mut total_rx = 0.0;
        let mut total_tx = 0.0;
        let mut any = false;
        for (name, c) in parse_net_dev(text) {
            let idx = match self.index.get(&name) {
                Some(&i) => i,
                None => {
                    let i = self.state.interfaces.len();
                    self.state
                        .interfaces
                        .push(Interface::new(name.clone(), self.history_len));
                    self.index.insert(name, i);
                    i
                }
            };
            let it = &mut self.state.interfaces[idx];
            it.rx_total = c.rx_bytes;
            it.tx_total = c.tx_bytes;
            it.errors = c.rx_errs + c.tx_errs;
            it.drops = c.rx_drop + c.tx_drop;
            if let Some(r) = it.rx_ctr.update(c.rx_bytes, now) {
                it.rx_rate = Reading::exact(r, Unit::BytesPerSec).with_id("net.throughput");
                it.rx_history.push(r);
                if !it.virtual_iface {
                    total_rx += r;
                    any = true;
                }
            }
            if let Some(t) = it.tx_ctr.update(c.tx_bytes, now) {
                it.tx_rate = Reading::exact(t, Unit::BytesPerSec).with_id("net.throughput");
                it.tx_history.push(t);
                if !it.virtual_iface {
                    total_tx += t;
                }
            }
        }
        if any {
            self.state.total_rx =
                Reading::exact(total_rx, Unit::BytesPerSec).with_id("net.throughput");
            self.state.total_tx =
                Reading::exact(total_tx, Unit::BytesPerSec).with_id("net.throughput");
            self.state.rx_history.push(total_rx);
            self.state.tx_history.push(total_tx);
        }
    }

    fn read_link_info(&mut self) {
        for it in &mut self.state.interfaces {
            let base = format!("/sys/class/net/{}", it.name);
            it.up = util::read_trimmed(format!("{base}/operstate")).as_deref() == Ok("up");
            it.mac = util::read_trimmed(format!("{base}/address"))
                .ok()
                .filter(|s| !s.is_empty());
            // speed 對未連線或虛擬介面會回 -1 或 EINVAL，這是正常的
            it.speed_mbit = util::read_i64_opt(format!("{base}/speed")).filter(|&v| v > 0);
        }
    }

    /// 用 getifaddrs(3) 讀 IP，不 fork 外部指令。
    fn read_addrs(&mut self) {
        for it in &mut self.state.interfaces {
            it.addrs.clear();
        }
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: 傳入合法的 out 參數；成功時保證要 freeifaddrs。
        if unsafe { libc::getifaddrs(&mut head) } != 0 {
            return;
        }
        let mut cur = head;
        while !cur.is_null() {
            // SAFETY: cur 由 getifaddrs 配置，在 freeifaddrs 之前都有效。
            let ifa = unsafe { &*cur };
            cur = ifa.ifa_next;
            if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() {
                continue;
            }
            // SAFETY: ifa_name 是核心提供的 NUL 結尾字串。
            let name = match unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) }.to_str() {
                Ok(n) => n,
                Err(_) => continue,
            };
            let Some(&idx) = self.index.get(name) else {
                continue;
            };
            // SAFETY: 只有在 sa_family 對得上時才把 sockaddr 當成對應型別。
            let family = unsafe { (*ifa.ifa_addr).sa_family } as i32;
            let Some(ip) = format_addr(ifa.ifa_addr, family) else {
                continue;
            };
            if family == libc::AF_INET6 && ip.starts_with("fe80") {
                continue; // link-local 沒有參考價值
            }
            let prefix = prefix_len(ifa.ifa_netmask, family);
            let text = match prefix {
                Some(p) => format!("{ip}/{p}"),
                None => ip,
            };
            self.state.interfaces[idx].addrs.push(text);
        }
        // SAFETY: head 由 getifaddrs 配置且尚未釋放。
        unsafe { libc::freeifaddrs(head) };
    }
}

fn format_addr(sa: *const libc::sockaddr, family: i32) -> Option<String> {
    let mut buf = [0i8; libc::NI_MAXHOST as usize];
    let len = match family {
        libc::AF_INET => std::mem::size_of::<libc::sockaddr_in>(),
        libc::AF_INET6 => std::mem::size_of::<libc::sockaddr_in6>(),
        _ => return None,
    } as libc::socklen_t;
    // SAFETY: sa 指向至少 len 位元組的合法 sockaddr；buf 大小正確。
    let rc = unsafe {
        libc::getnameinfo(
            sa,
            len,
            buf.as_mut_ptr(),
            buf.len() as libc::socklen_t,
            std::ptr::null_mut(),
            0,
            libc::NI_NUMERICHOST,
        )
    };
    if rc != 0 {
        return None;
    }
    // SAFETY: getnameinfo 成功時保證寫入 NUL 結尾字串。
    let s = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }
        .to_str()
        .ok()?;
    // IPv6 的 scope id（fe80::1%eth0）對顯示沒意義
    Some(s.split('%').next().unwrap_or(s).to_owned())
}

fn prefix_len(mask: *const libc::sockaddr, family: i32) -> Option<u32> {
    if mask.is_null() {
        return None;
    }
    // SAFETY: 依 family 決定要讀的位元組數，不超過該型別大小。
    let bits: u32 = unsafe {
        match family {
            libc::AF_INET => {
                let m = &*(mask as *const libc::sockaddr_in);
                m.sin_addr.s_addr.count_ones()
            }
            libc::AF_INET6 => {
                let m = &*(mask as *const libc::sockaddr_in6);
                m.sin6_addr.s6_addr.iter().map(|b| b.count_ones()).sum()
            }
            _ => return None,
        }
    };
    (bits > 0).then_some(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_net_dev_line() {
        let (n, c) =
            parse_net_dev_line("  eth0: 1000 10 1 2 0 0 0 0  2000 20 3 4 0 0 0 0").unwrap();
        assert_eq!(n, "eth0");
        assert_eq!(c.rx_bytes, 1000);
        assert_eq!(c.tx_bytes, 2000);
        assert_eq!(c.rx_errs, 1);
        assert_eq!(c.tx_drop, 4);
    }

    #[test]
    fn handles_missing_space_after_colon() {
        // 流量大時核心不會在冒號後補空白，欄位會黏在一起
        let (n, c) =
            parse_net_dev_line("eno1:184467440737 10 0 0 0 0 0 0 2000 20 0 0 0 0 0 0").unwrap();
        assert_eq!(n, "eno1");
        assert_eq!(c.rx_bytes, 184467440737);
    }

    #[test]
    fn skips_header_lines() {
        let text = "Inter-|   Receive\n face |bytes packets\n  lo: 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16\n";
        let v = parse_net_dev(text);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "lo");
    }

    #[test]
    fn rejects_truncated_lines() {
        assert!(parse_net_dev_line("eth0: 1 2 3").is_none());
        assert!(parse_net_dev_line("no colon here").is_none());
        assert!(parse_net_dev_line(": 1 2 3").is_none());
    }

    #[test]
    fn classifies_virtual_interfaces() {
        assert!(is_virtual_interface("lo"), "lo 計入總量會讓數字虛胖一倍");
        assert!(is_virtual_interface("docker0"));
        assert!(is_virtual_interface("veth1a2b3c"));
        assert!(!is_virtual_interface("eno1"));
        assert!(!is_virtual_interface("wlp0s20f3"));
    }

    #[test]
    fn counter_rejects_wraparound() {
        let mut c = Counter::new();
        let t0 = Instant::now();
        assert_eq!(c.update(1000, t0), None, "第一次沒有基準點");
        let t1 = t0 + std::time::Duration::from_secs(1);
        assert_eq!(c.update(2000, t1), Some(1000.0));
        let t2 = t1 + std::time::Duration::from_secs(1);
        assert_eq!(c.update(5, t2), None, "32 位元計數器回捲時不可回傳巨大速率");
    }

    #[test]
    fn parses_sockstat() {
        let s = parse_sockstat(
            "sockets: used 1234\n\
             TCP: inuse 40 orphan 2 tw 15 alloc 50 mem 6\n\
             UDP: inuse 12 mem 3\n",
        );
        assert_eq!(s.sockets_used, Some(1234));
        assert_eq!(s.tcp_inuse, Some(40));
        assert_eq!(s.tcp_tw, Some(15));
        assert_eq!(s.tcp_orphan, Some(2));
        assert_eq!(s.udp_inuse, Some(12));
    }

    #[test]
    fn sockstat_missing_fields_are_none() {
        let s = parse_sockstat("TCP: alloc 5\n");
        assert_eq!(s.tcp_inuse, None);
    }
}
