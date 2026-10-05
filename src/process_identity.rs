//! Read-only local evidence. Multiple Sunshine processes are deliberately ambiguous.
#[cfg(target_os = "linux")]
pub fn sunshine_generation(port: u16) -> Option<String> {
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let mut sockets = std::collections::BTreeSet::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(text) = std::fs::read_to_string(table) {
            for line in text.lines().skip(1) {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.len() > 9
                    && fields[3] == "0A"
                    && fields[1]
                        .rsplit_once(':')
                        .and_then(|(_, p)| u16::from_str_radix(p, 16).ok())
                        == Some(port)
                {
                    sockets.insert(format!("socket:[{}]", fields[9]));
                }
            }
        }
    }
    if sockets.is_empty() {
        return None;
    }
    let mut found = None;
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let path = entry.path();
        let Ok(exe) = std::fs::read_link(path.join("exe")) else {
            continue;
        };
        if !exe.file_name().is_some_and(|name| name == "sunshine") {
            continue;
        }
        let owns_listener = std::fs::read_dir(path.join("fd"))
            .ok()
            .is_some_and(|entries| {
                entries.flatten().any(|fd| {
                    std::fs::read_link(fd.path())
                        .ok()
                        .is_some_and(|target| sockets.contains(target.to_string_lossy().as_ref()))
                })
            });
        if !owns_listener {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(path.join("stat")) else {
            continue;
        };
        // Field 22 is birth ticks; the parenthesized process name may contain spaces.
        let birth = stat.rsplit_once(')')?.1.split_whitespace().nth(19)?;
        let generation = format!("{}:{pid}:{birth}", boot.trim());
        if found.is_some() {
            return None;
        }
        found = Some(generation);
    }
    found
}

#[cfg(target_os = "windows")]
pub fn sunshine_generation(port: u16) -> Option<String> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        },
    };
    struct Handle(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return None;
        }
        let snapshot = Handle(snapshot);
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut present = Process32FirstW(snapshot.0, &mut entry);
        let owners = listener_owners(port)?;
        let mut found = None;
        while present != 0 {
            let end = entry
                .szExeFile
                .iter()
                .position(|v| *v == 0)
                .unwrap_or(entry.szExeFile.len());
            if owners.contains(&entry.th32ProcessID)
                && String::from_utf16_lossy(&entry.szExeFile[..end])
                    .eq_ignore_ascii_case("sunshine.exe")
            {
                if found.is_some() {
                    return None;
                }
                let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID);
                if handle.is_null() {
                    return None;
                }
                let handle = Handle(handle);
                let mut birth: FILETIME = std::mem::zeroed();
                let mut exit = birth;
                let mut kernel = birth;
                let mut user = birth;
                if GetProcessTimes(handle.0, &mut birth, &mut exit, &mut kernel, &mut user) == 0 {
                    return None;
                }
                let ticks =
                    (u64::from(birth.dwHighDateTime) << 32) | u64::from(birth.dwLowDateTime);
                found = Some(format!("{}:{ticks}", entry.th32ProcessID));
            }
            present = Process32NextW(snapshot.0, &mut entry);
        }
        found
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn sunshine_generation(_port: u16) -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
fn listener_owners(port: u16) -> Option<std::collections::BTreeSet<u32>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    let mut owners = std::collections::BTreeSet::new();
    for (family, row_size, port_offset, pid_offset) in
        [(AF_INET, 24usize, 8usize, 20usize), (AF_INET6, 56, 20, 52)]
    {
        let mut size = 0u32;
        unsafe {
            GetExtendedTcpTable(
                std::ptr::null_mut(),
                &mut size,
                0,
                family as u32,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            );
        }
        if size == 0 || size > 1024 * 1024 {
            return None;
        }
        let mut words = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        let result = unsafe {
            GetExtendedTcpTable(
                words.as_mut_ptr().cast(),
                &mut size,
                0,
                family as u32,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if result != 0 || size as usize > words.len() * std::mem::size_of::<usize>() {
            return None;
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), size as usize) };
        let read = |at: usize| -> Option<u32> {
            Some(u32::from_ne_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
        };
        let rows = read(0)? as usize;
        if 4 + rows.checked_mul(row_size)? > bytes.len() {
            return None;
        }
        for row in 0..rows {
            let start = 4 + row * row_size;
            if (read(start + port_offset)? as u16).swap_bytes() == port {
                owners.insert(read(start + pid_offset)?);
            }
        }
    }
    Some(owners)
}
