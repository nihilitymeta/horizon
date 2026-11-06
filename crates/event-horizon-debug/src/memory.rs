/// The process's resident and virtual memory, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryUsage {
    pub resident: u64,
    pub virtual_size: u64,
}

/// The game's memory footprint: `/proc/self/status` on Linux (`None` on
/// platforms without it, where the overlay leaves the line out).
pub fn memory_usage() -> Option<MemoryUsage> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        parse_proc_status(&status)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_proc_status(status: &str) -> Option<MemoryUsage> {
    let mut usage = MemoryUsage::default();
    let mut seen = false;
    for line in status.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let Some(kib) = value
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|kib| kib * 1024)
        else {
            continue;
        };
        match key {
            "VmRSS" => {
                usage.resident = kib;
                seen = true;
            }
            "VmSize" => {
                usage.virtual_size = kib;
                seen = true;
            }
            _ => {}
        }
    }
    seen.then_some(usage)
}

/// `482 MB` below a gibibyte, `1.8 GB` above it.
pub fn memory_label(bytes: u64) -> String {
    const GIB: u64 = 1 << 30;
    if bytes >= GIB {
        format!("{:.1} GB", bytes as f64 / GIB as f64)
    } else {
        format!("{} MB", bytes >> 20)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_lines_read_proc_status_and_format_like_the_design() {
        let status = "Name:\thorizon-client\nVmSize:\t 1887436 kB\n\
                      VmRSS:\t  493568 kB\nThreads:\t42\n";
        assert_eq!(
            parse_proc_status(status),
            Some(MemoryUsage {
                resident: 493568 * 1024,
                virtual_size: 1887436 * 1024,
            })
        );
        assert_eq!(parse_proc_status("Name:\tgame\n"), None);
        assert_eq!(memory_label(482 * 1024 * 1024), "482 MB");
        assert_eq!(memory_label(1887436 * 1024), "1.8 GB");
    }
}
