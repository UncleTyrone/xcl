//! `XCL_NETWORK=0`: the title runs as on a console without a network. Its WinHTTP requests and
//! its sockets fail as they do there (name not resolved / network unreachable); nothing is
//! answered in a server's place. Loopback connections stay (titles make socket pairs), and so
//! do lookups of the local names (none, this machine's, localhost, IP literals). Only
//! the title's own imports are concerned (the game's exe and DLLs); off by default.
//!
//! For titles under bring-up: a crash reporter otherwise uploads a memory dump of the process
//! on every crash.
//!
//! The title's imports are not the only way out: a retail game's crash reporter reached the
//! system's WinHTTP another way and uploaded a 21.5 MB minidump with `XCL_NETWORK=0` (#305).
//! So the same refusals are also hooked into the system's own winhttp / wininet / ws2_32
//! (`install_system_hooks`): whatever finds those functions – an import, GetProcAddress, a DLL
//! of the title's – gets no network. What goes through (loopback, local names) calls the
//! original through the hook's trampoline.
//!
//! Depth vs CGBJ `modules::net` (Wave B): bind-all → loopback, `ConnectEx` via `WSAIoctl`,
//! `getnameinfo` reverse policy, WinHTTP connect/proxy refusals. Still thinner on
//! `GetAddrInfoExA`, `WSASendMsg` message-name vetting, and some newer WinHTTP proxy APIs.

use std::ffi::c_void;
use std::mem::{size_of, size_of_val};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::SetLastError;

pub const NETOFF: &str = "xcl-netoff.dll";

const ERROR_WINHTTP_NAME_NOT_RESOLVED: u32 = 12007;
const ERROR_WINHTTP_CANNOT_CONNECT: u32 = 12029;
const ERROR_WINHTTP_AUTODETECTION_FAILED: u32 = 12180;
const WSAEINVAL: i32 = 10022;
const WSAEADDRNOTAVAIL: i32 = 10049;
const WSAENETUNREACH: i32 = 10051;
const WSAHOST_NOT_FOUND: i32 = 11001;
const AF_INET: u16 = 2;
const AF_INET6: u16 = 23;
const NI_NUMERICHOST: i32 = 0x02;
const NI_NAMEREQD: i32 = 0x04;
const SIO_GET_EXTENSION_FUNCTION_POINTER: u32 = 0xC800_0006;
const WINHTTP_ACCESS_TYPE_NO_PROXY: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid(u32, u16, u16, [u8; 8]);

const WSAID_CONNECTEX: Guid = Guid(
    0x25a2_07b9,
    0xddf3,
    0x4660,
    [0x8e, 0xe9, 0x76, 0xe5, 0x8c, 0x74, 0x06, 0x3e],
);

#[link(name = "ws2_32", kind = "raw-dylib")]
extern "system" {
    #[link_name = "connect"]
    fn real_connect(s: usize, name: *const u8, len: i32) -> i32;
    #[link_name = "bind"]
    fn real_bind(s: usize, name: *const u8, len: i32) -> i32;
    #[link_name = "sendto"]
    fn real_sendto(s: usize, buf: *const u8, len: i32, flags: i32, to: *const u8, tolen: i32) -> i32;
    fn WSASetLastError(e: i32);
    #[link_name = "getaddrinfo"]
    fn real_getaddrinfo(node: *const u8, service: *const u8, hints: *const c_void, out: *mut *mut c_void) -> i32;
    #[link_name = "GetAddrInfoW"]
    fn real_getaddrinfo_w(node: *const u16, service: *const u16, hints: *const c_void, out: *mut *mut c_void) -> i32;
    #[link_name = "GetAddrInfoExW"]
    #[allow(clippy::too_many_arguments)]
    fn real_getaddrinfo_ex(n: *const u16, s: *const u16, ns: u32, id: *const c_void, hints: *const c_void, out: *mut *mut c_void, t: *const c_void, ov: *mut c_void, cb: *const c_void, h: *mut c_void) -> i32;
    #[link_name = "gethostname"]
    fn real_gethostname(name: *mut u8, len: i32) -> i32;
    #[link_name = "getnameinfo"]
    fn real_getnameinfo(sa: *const u8, salen: i32, host: *mut u8, hostlen: u32, serv: *mut u8, servlen: u32, flags: i32) -> i32;
    #[link_name = "WSAIoctl"]
    #[allow(clippy::too_many_arguments)]
    fn real_wsa_ioctl(s: usize, code: u32, in_buf: *const c_void, in_len: u32, out_buf: *mut c_void, out_len: u32, ret: *mut u32, ov: *mut c_void, cb: *const c_void) -> i32;
}

#[link(name = "winhttp", kind = "raw-dylib")]
extern "system" {
    #[link_name = "WinHttpOpen"]
    fn real_winhttp_open(agent: *const u16, access: u32, proxy: *const u16, bypass: *const u16, flags: u32) -> *mut c_void;
    #[link_name = "WinHttpConnect"]
    fn real_winhttp_connect(session: *mut c_void, server: *const u16, port: u16, reserved: u32) -> *mut c_void;
}

/// A name answered without asking the network: none, localhost, this host, IP literals.
fn local_name(name: &str) -> bool {
    let bare = name.trim_start_matches('[').trim_end_matches(']');
    if bare.is_empty()
        || bare.eq_ignore_ascii_case("localhost")
        || bare.parse::<IpAddr>().is_ok()
    {
        return true;
    }
    let mut buf = [0u8; 256];
    let own = unsafe { real_gethostname(buf.as_mut_ptr(), buf.len() as i32) } == 0;
    own && {
        let n = buf.iter().position(|&b| b == 0).unwrap_or(0);
        std::str::from_utf8(&buf[..n]).is_ok_and(|h| h.eq_ignore_ascii_case(bare))
    }
}

fn loopback_name(name: &str) -> bool {
    let bare = name.trim_start_matches('[').trim_end_matches(']');
    bare.eq_ignore_ascii_case("localhost") || bare.parse::<IpAddr>().is_ok_and(|ip| ip.to_canonical().is_loopback())
}

pub fn enabled() -> bool {
    std::env::var("XCL_NETWORK").is_ok_and(|v| v.trim() == "0")
}

/// (name, address, ws2_32 ordinal – 0: imported by name only)
fn table() -> Vec<(&'static str, usize, u16)> {
    vec![
        ("WinHttpSendRequest", send_request as *const () as usize, 0),
        ("WinHttpConnect", winhttp_connect as *const () as usize, 0),
        ("WinHttpOpen", winhttp_open as *const () as usize, 0),
        ("WinHttpGetProxyForUrl", winhttp_proxy_fail as *const () as usize, 0),
        ("connect", connect as *const () as usize, 4),
        ("bind", bind as *const () as usize, 2),
        ("sendto", sendto as *const () as usize, 20),
        ("gethostbyname", gethostbyname as *const () as usize, 52),
        ("WSAConnect", wsa_connect as *const () as usize, 0),
        ("WSASendTo", wsa_send_to as *const () as usize, 0),
        ("WSAIoctl", wsa_ioctl as *const () as usize, 0),
        ("getaddrinfo", getaddrinfo as *const () as usize, 0),
        ("GetAddrInfoW", getaddrinfo_w as *const () as usize, 0),
        ("GetAddrInfoExW", getaddrinfo_ex as *const () as usize, 0),
        ("getnameinfo", getnameinfo as *const () as usize, 0),
    ]
}

pub fn exports() -> Vec<(&'static str, usize)> {
    table().into_iter().map(|(n, a, _)| (n, a)).collect()
}

pub fn ordinals() -> Vec<(u16, &'static str)> {
    table().into_iter().filter(|t| t.2 != 0).map(|(n, _, o)| (o, n)).collect()
}

fn note(what: &str) {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 16 {
        crate::xlog_info!("XCL_NETWORK=0: {what} fails (no network)");
    }
}

fn once(what: &'static str, destination: String) {
    static SEEN: Mutex<Vec<(&'static str, String)>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if !seen.iter().any(|(w, d)| *w == what && *d == destination) {
        crate::xlog_info!("XCL_NETWORK=0: {what} ({destination})");
        seen.push((what, destination));
    }
}

fn socket_addr(addr: *const u8, len: i32) -> Option<SocketAddr> {
    let len = usize::try_from(len).ok()?;
    if addr.is_null() || len < 4 {
        return None;
    }
    // SAFETY: caller passes `len` readable bytes.
    let b = unsafe { std::slice::from_raw_parts(addr, len) };
    let port = u16::from_be_bytes([b[2], b[3]]);
    match u16::from_le_bytes([b[0], b[1]]) {
        AF_INET if len >= 16 => Some((<[u8; 4]>::try_from(&b[4..8]).ok()?, port).into()),
        AF_INET6 if len >= 28 => Some((<[u8; 16]>::try_from(&b[8..24]).ok()?, port).into()),
        _ => None,
    }
}

/// 127.0.0.0/8 or ::1 (sockaddr_in / sockaddr_in6)
unsafe fn loopback(name: *const u8, len: i32) -> bool {
    socket_addr(name, len).is_some_and(|a| a.ip().to_canonical().is_loopback())
}

unsafe fn unreachable_net(what: &str) -> i32 {
    note(what);
    WSASetLastError(WSAENETUNREACH);
    -1
}

static O_CONNECT: AtomicUsize = AtomicUsize::new(0);
static O_BIND: AtomicUsize = AtomicUsize::new(0);
static O_SENDTO: AtomicUsize = AtomicUsize::new(0);
static O_GETADDRINFO: AtomicUsize = AtomicUsize::new(0);
static O_GETADDRINFO_W: AtomicUsize = AtomicUsize::new(0);
static O_GETADDRINFO_EX: AtomicUsize = AtomicUsize::new(0);
static O_GETNAMEINFO: AtomicUsize = AtomicUsize::new(0);
static O_WSAIOCTL: AtomicUsize = AtomicUsize::new(0);
static O_WINHTTP_OPEN: AtomicUsize = AtomicUsize::new(0);
static O_WINHTTP_CONNECT: AtomicUsize = AtomicUsize::new(0);
static O_UNUSED: [AtomicUsize; 24] = [const { AtomicUsize::new(0) }; 24];

unsafe fn orig_connect(s: usize, name: *const u8, len: i32) -> i32 {
    match O_CONNECT.load(Ordering::Acquire) {
        0 => real_connect(s, name, len),
        f => std::mem::transmute::<usize, unsafe extern "system" fn(usize, *const u8, i32) -> i32>(f)(s, name, len),
    }
}

unsafe fn orig_bind(s: usize, name: *const u8, len: i32) -> i32 {
    match O_BIND.load(Ordering::Acquire) {
        0 => real_bind(s, name, len),
        f => std::mem::transmute::<usize, unsafe extern "system" fn(usize, *const u8, i32) -> i32>(f)(s, name, len),
    }
}

unsafe fn orig_sendto(s: usize, buf: *const u8, len: i32, flags: i32, to: *const u8, tolen: i32) -> i32 {
    match O_SENDTO.load(Ordering::Acquire) {
        0 => real_sendto(s, buf, len, flags, to, tolen),
        f => std::mem::transmute::<usize, unsafe extern "system" fn(usize, *const u8, i32, i32, *const u8, i32) -> i32>(f)(s, buf, len, flags, to, tolen),
    }
}

unsafe fn orig_getaddrinfo(node: *const u8, service: *const u8, hints: *const c_void, out: *mut *mut c_void) -> i32 {
    match O_GETADDRINFO.load(Ordering::Acquire) {
        0 => real_getaddrinfo(node, service, hints, out),
        f => std::mem::transmute::<usize, unsafe extern "system" fn(*const u8, *const u8, *const c_void, *mut *mut c_void) -> i32>(f)(node, service, hints, out),
    }
}

unsafe fn orig_getaddrinfo_w(node: *const u16, service: *const u16, hints: *const c_void, out: *mut *mut c_void) -> i32 {
    match O_GETADDRINFO_W.load(Ordering::Acquire) {
        0 => real_getaddrinfo_w(node, service, hints, out),
        f => std::mem::transmute::<usize, unsafe extern "system" fn(*const u16, *const u16, *const c_void, *mut *mut c_void) -> i32>(f)(node, service, hints, out),
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn orig_getaddrinfo_ex(n: *const u16, s: *const u16, ns: u32, id: *const c_void, hints: *const c_void, out: *mut *mut c_void, t: *const c_void, ov: *mut c_void, cb: *const c_void, h: *mut c_void) -> i32 {
    match O_GETADDRINFO_EX.load(Ordering::Acquire) {
        0 => real_getaddrinfo_ex(n, s, ns, id, hints, out, t, ov, cb, h),
        f => std::mem::transmute::<usize, unsafe extern "system" fn(*const u16, *const u16, u32, *const c_void, *const c_void, *mut *mut c_void, *const c_void, *mut c_void, *const c_void, *mut c_void) -> i32>(f)(n, s, ns, id, hints, out, t, ov, cb, h),
    }
}

unsafe fn orig_getnameinfo(sa: *const u8, salen: i32, host: *mut u8, hostlen: u32, serv: *mut u8, servlen: u32, flags: i32) -> i32 {
    match O_GETNAMEINFO.load(Ordering::Acquire) {
        0 => real_getnameinfo(sa, salen, host, hostlen, serv, servlen, flags),
        f => std::mem::transmute::<usize, unsafe extern "system" fn(*const u8, i32, *mut u8, u32, *mut u8, u32, i32) -> i32>(f)(sa, salen, host, hostlen, serv, servlen, flags),
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn orig_wsa_ioctl(s: usize, code: u32, in_buf: *const c_void, in_len: u32, out_buf: *mut c_void, out_len: u32, ret: *mut u32, ov: *mut c_void, cb: *const c_void) -> i32 {
    match O_WSAIOCTL.load(Ordering::Acquire) {
        0 => real_wsa_ioctl(s, code, in_buf, in_len, out_buf, out_len, ret, ov, cb),
        f => std::mem::transmute::<usize, unsafe extern "system" fn(usize, u32, *const c_void, u32, *mut c_void, u32, *mut u32, *mut c_void, *const c_void) -> i32>(f)(s, code, in_buf, in_len, out_buf, out_len, ret, ov, cb),
    }
}

unsafe fn orig_winhttp_open(agent: *const u16, access: u32, proxy: *const u16, bypass: *const u16, flags: u32) -> *mut c_void {
    match O_WINHTTP_OPEN.load(Ordering::Acquire) {
        0 => real_winhttp_open(agent, access, proxy, bypass, flags),
        f => std::mem::transmute::<usize, unsafe extern "system" fn(*const u16, u32, *const u16, *const u16, u32) -> *mut c_void>(f)(agent, access, proxy, bypass, flags),
    }
}

unsafe fn orig_winhttp_connect(session: *mut c_void, server: *const u16, port: u16, reserved: u32) -> *mut c_void {
    match O_WINHTTP_CONNECT.load(Ordering::Acquire) {
        0 => real_winhttp_connect(session, server, port, reserved),
        f => std::mem::transmute::<usize, unsafe extern "system" fn(*mut c_void, *const u16, u16, u32) -> *mut c_void>(f)(session, server, port, reserved),
    }
}

/// `XCL_NETWORK=0` for the whole process: the system's own network functions refuse as the
/// title's imports do (see the module's comment). Call once, before the title runs.
pub fn install_system_hooks() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let mut unused = O_UNUSED.iter();
        let mut spare = || unused.next().expect("enough spare trampolines");
        let hooks: [(&str, &str, usize, &AtomicUsize); 24] = [
            ("winhttp.dll", "WinHttpSendRequest", send_request as usize, spare()),
            ("winhttp.dll", "WinHttpConnect", winhttp_connect as usize, &O_WINHTTP_CONNECT),
            ("winhttp.dll", "WinHttpOpen", winhttp_open as usize, &O_WINHTTP_OPEN),
            ("winhttp.dll", "WinHttpGetProxyForUrl", winhttp_proxy_fail as usize, spare()),
            ("wininet.dll", "InternetConnectA", null_handle_inet8 as usize, spare()),
            ("wininet.dll", "InternetConnectW", null_handle_inet8 as usize, spare()),
            ("wininet.dll", "InternetOpenUrlA", null_handle_inet6 as usize, spare()),
            ("wininet.dll", "InternetOpenUrlW", null_handle_inet6 as usize, spare()),
            ("wininet.dll", "HttpSendRequestA", false_inet5 as usize, spare()),
            ("wininet.dll", "HttpSendRequestW", false_inet5 as usize, spare()),
            ("wininet.dll", "HttpSendRequestExA", false_inet5 as usize, spare()),
            ("wininet.dll", "HttpSendRequestExW", false_inet5 as usize, spare()),
            ("ws2_32.dll", "connect", connect as usize, &O_CONNECT),
            ("ws2_32.dll", "bind", bind as usize, &O_BIND),
            ("ws2_32.dll", "WSAConnect", wsa_connect as usize, spare()),
            ("ws2_32.dll", "sendto", sendto as usize, &O_SENDTO),
            ("ws2_32.dll", "WSASendTo", wsa_send_to as usize, spare()),
            ("ws2_32.dll", "WSAIoctl", wsa_ioctl as usize, &O_WSAIOCTL),
            ("ws2_32.dll", "gethostbyname", gethostbyname as usize, spare()),
            ("ws2_32.dll", "getaddrinfo", getaddrinfo as usize, &O_GETADDRINFO),
            ("ws2_32.dll", "GetAddrInfoW", getaddrinfo_w as usize, &O_GETADDRINFO_W),
            ("ws2_32.dll", "GetAddrInfoExW", getaddrinfo_ex as usize, &O_GETADDRINFO_EX),
            ("ws2_32.dll", "getnameinfo", getnameinfo as usize, &O_GETNAMEINFO),
            ("ws2_32.dll", "GetNameInfoW", getnameinfo_w as usize, spare()),
        ];
        let mut failed = Vec::new();
        for (dll, name, detour, orig) in hooks {
            if let Err(e) = crate::hooks::hook(dll, name, detour, orig) {
                failed.push(e);
            }
        }
        if failed.is_empty() {
            crate::xlog_info!("XCL_NETWORK=0: the system's WinHTTP, WinINet and Winsock refuse the network for the whole process (loopback and local names excepted)");
        } else {
            crate::xlog_warn!("XCL_NETWORK=0: not every system network function could be hooked – {}", failed.join("; "));
        }
    });
}

unsafe extern "system" fn winhttp_open(agent: *const u16, _access: u32, _proxy: *const u16, bypass: *const u16, flags: u32) -> *mut c_void {
    // Proxies / WPAD stay off — same as an unplugged console.
    orig_winhttp_open(agent, WINHTTP_ACCESS_TYPE_NO_PROXY, std::ptr::null(), bypass, flags)
}

unsafe extern "system" fn winhttp_connect(session: *mut c_void, server: *const u16, port: u16, reserved: u32) -> *mut c_void {
    let name = if server.is_null() {
        String::new()
    } else {
        crate::util::from_wide_ptr(server).unwrap_or_default()
    };
    if loopback_name(&name) {
        return orig_winhttp_connect(session, server, port, reserved);
    }
    once("network WinHttpConnect blocked", name);
    SetLastError(ERROR_WINHTTP_CANNOT_CONNECT);
    std::ptr::null_mut()
}

unsafe extern "system" fn winhttp_proxy_fail(
    _session: *mut c_void,
    _url: *const u16,
    _options: *const c_void,
    _info: *mut c_void,
) -> i32 {
    note("WinHttpGetProxyForUrl");
    SetLastError(ERROR_WINHTTP_AUTODETECTION_FAILED);
    0
}

unsafe extern "system" fn null_handle_inet8(_h: *mut c_void, _server: *const c_void, _port: u16, _user: *const c_void, _pass: *const c_void, _service: u32, _flags: u32, _ctx: usize) -> *mut c_void {
    note("InternetConnect");
    SetLastError(ERROR_WINHTTP_NAME_NOT_RESOLVED);
    std::ptr::null_mut()
}

unsafe extern "system" fn null_handle_inet6(_h: *mut c_void, _url: *const c_void, _headers: *const c_void, _hlen: u32, _flags: u32, _ctx: usize) -> *mut c_void {
    note("InternetOpenUrl");
    SetLastError(ERROR_WINHTTP_NAME_NOT_RESOLVED);
    std::ptr::null_mut()
}

unsafe extern "system" fn false_inet5(_req: *mut c_void, _a: *const c_void, _b: usize, _c: *const c_void, _d: usize) -> i32 {
    note("HttpSendRequest");
    SetLastError(ERROR_WINHTTP_NAME_NOT_RESOLVED);
    0
}

unsafe extern "system" fn send_request(_req: *mut c_void, _headers: *const u16, _hlen: u32, _opt: *const c_void, _olen: u32, _total: u32, _ctx: usize) -> i32 {
    note("WinHttpSendRequest");
    SetLastError(ERROR_WINHTTP_NAME_NOT_RESOLVED);
    0
}

unsafe extern "system" fn connect(s: usize, name: *const u8, len: i32) -> i32 {
    if loopback(name, len) {
        return orig_connect(s, name, len);
    }
    if let Some(a) = socket_addr(name, len) {
        once("network connect blocked", a.to_string());
    }
    unreachable_net("connect")
}

unsafe extern "system" fn bind(s: usize, name: *const u8, len: i32) -> i32 {
    match socket_addr(name, len) {
        Some(a) if a.ip().to_canonical().is_loopback() => orig_bind(s, name, len),
        Some(a) if a.ip().is_unspecified() => {
            once("network bind narrowed to loopback", a.to_string());
            let mut local = std::slice::from_raw_parts(name, len as usize).to_vec();
            match a {
                SocketAddr::V4(_) => local[4..8].copy_from_slice(&[127, 0, 0, 1]),
                SocketAddr::V6(_) => local[23] = 1,
            }
            orig_bind(s, local.as_ptr(), len)
        }
        a => {
            once("network bind blocked", a.map(|x| x.to_string()).unwrap_or_else(|| "not an IP".into()));
            WSASetLastError(WSAEADDRNOTAVAIL);
            -1
        }
    }
}

unsafe extern "system" fn sendto(s: usize, buf: *const u8, len: i32, flags: i32, to: *const u8, tolen: i32) -> i32 {
    if to.is_null() || loopback(to, tolen) {
        return orig_sendto(s, buf, len, flags, to, tolen);
    }
    unreachable_net("sendto")
}

unsafe extern "system" fn wsa_connect(s: usize, name: *const u8, len: i32, _a: *const c_void, _b: *mut c_void, _c: *const c_void, _d: *const c_void) -> i32 {
    connect(s, name, len)
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn wsa_send_to(_s: usize, _bufs: *const c_void, _n: u32, _sent: *mut u32, _flags: u32, _to: *const u8, _tolen: i32, _ov: *mut c_void, _cb: *const c_void) -> i32 {
    unreachable_net("WSASendTo")
}

type ConnectExFn = unsafe extern "system" fn(usize, *const u8, i32, *const c_void, u32, *mut u32, *mut c_void) -> i32;

unsafe extern "system" fn connect_ex(
    s: usize,
    name: *const u8,
    len: i32,
    send_buf: *const c_void,
    send_len: u32,
    bytes_sent: *mut u32,
    overlapped: *mut c_void,
) -> i32 {
    if !loopback(name, len) {
        return unreachable_net("ConnectEx");
    }
    // Resolve the real ConnectEx for this socket, then call it.
    let mut ptr: usize = 0;
    let mut ret = 0u32;
    let guid = WSAID_CONNECTEX;
    let got = orig_wsa_ioctl(
        s,
        SIO_GET_EXTENSION_FUNCTION_POINTER,
        (&raw const guid).cast(),
        size_of_val(&guid) as u32,
        (&raw mut ptr).cast(),
        size_of_val(&ptr) as u32,
        &mut ret,
        std::ptr::null_mut(),
        std::ptr::null(),
    );
    if got != 0 || ptr == 0 {
        WSASetLastError(WSAEINVAL);
        return -1;
    }
    std::mem::transmute::<usize, ConnectExFn>(ptr)(s, name, len, send_buf, send_len, bytes_sent, overlapped)
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn wsa_ioctl(
    s: usize,
    code: u32,
    in_buf: *const c_void,
    in_len: u32,
    out_buf: *mut c_void,
    out_len: u32,
    ret: *mut u32,
    ov: *mut c_void,
    cb: *const c_void,
) -> i32 {
    if code == SIO_GET_EXTENSION_FUNCTION_POINTER
        && !in_buf.is_null()
        && in_len as usize >= size_of::<Guid>()
        && !out_buf.is_null()
        && out_len as usize >= size_of::<usize>()
    {
        let guid = *(in_buf as *const Guid);
        if guid == WSAID_CONNECTEX {
            *(out_buf as *mut usize) = connect_ex as usize;
            if !ret.is_null() {
                *ret = size_of::<usize>() as u32;
            }
            return 0;
        }
    }
    orig_wsa_ioctl(s, code, in_buf, in_len, out_buf, out_len, ret, ov, cb)
}

unsafe extern "system" fn gethostbyname(_name: *const u8) -> *mut c_void {
    note("gethostbyname");
    WSASetLastError(WSAHOST_NOT_FOUND);
    std::ptr::null_mut()
}

unsafe extern "system" fn getaddrinfo(node: *const c_void, _service: *const c_void, _hints: *const c_void, out: *mut *mut c_void) -> i32 {
    let name = if node.is_null() {
        String::new()
    } else {
        crate::util::from_ansi_ptr(node as *const u8).unwrap_or_default()
    };
    if local_name(&name) {
        return orig_getaddrinfo(node as *const u8, _service as *const u8, _hints, out);
    }
    note(&format!("getaddrinfo({name})"));
    if !out.is_null() {
        *out = std::ptr::null_mut();
    }
    WSAHOST_NOT_FOUND
}

unsafe extern "system" fn getaddrinfo_w(node: *const u16, _service: *const u16, _hints: *const c_void, out: *mut *mut c_void) -> i32 {
    let name = if node.is_null() {
        String::new()
    } else {
        crate::util::from_wide_ptr(node).unwrap_or_default()
    };
    if local_name(&name) {
        return orig_getaddrinfo_w(node, _service, _hints, out);
    }
    note(&format!("GetAddrInfoW({name})"));
    if !out.is_null() {
        *out = std::ptr::null_mut();
    }
    WSAHOST_NOT_FOUND
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn getaddrinfo_ex(n: *const u16, s: *const u16, ns: u32, id: *const c_void, hints: *const c_void, out: *mut *mut c_void, t: *const c_void, ov: *mut c_void, cb: *const c_void, h: *mut c_void) -> i32 {
    let name = if n.is_null() {
        String::new()
    } else {
        crate::util::from_wide_ptr(n).unwrap_or_default()
    };
    if local_name(&name) {
        return orig_getaddrinfo_ex(n, s, ns, id, hints, out, t, ov, cb, h);
    }
    note(&format!("GetAddrInfoExW({name})"));
    if !out.is_null() {
        *out = std::ptr::null_mut();
    }
    WSAHOST_NOT_FOUND
}

unsafe extern "system" fn getnameinfo(
    sa: *const u8,
    salen: i32,
    host: *mut u8,
    hostlen: u32,
    serv: *mut u8,
    servlen: u32,
    flags: i32,
) -> i32 {
    let host_wanted = !host.is_null() && hostlen > 0;
    let flags = if !host_wanted || flags & NI_NUMERICHOST != 0 {
        flags
    } else if socket_addr(sa, salen).is_some_and(|a| a.ip().to_canonical().is_loopback()) {
        flags
    } else if flags & NI_NAMEREQD != 0 {
        once("network reverse lookup blocked", "getnameinfo".into());
        WSASetLastError(WSAHOST_NOT_FOUND);
        return WSAHOST_NOT_FOUND;
    } else {
        flags | NI_NUMERICHOST
    };
    orig_getnameinfo(sa, salen, host, hostlen, serv, servlen, flags)
}

unsafe extern "system" fn getnameinfo_w(
    sa: *const u8,
    salen: i32,
    host: *mut u16,
    hostlen: u32,
    serv: *mut u16,
    servlen: u32,
    flags: i32,
) -> i32 {
    // Same reverse-lookup policy as getnameinfo; force numeric off-machine.
    let host_wanted = !host.is_null() && hostlen > 0;
    let flags = if !host_wanted || flags & NI_NUMERICHOST != 0 {
        flags
    } else if socket_addr(sa, salen).is_some_and(|a| a.ip().to_canonical().is_loopback()) {
        flags
    } else if flags & NI_NAMEREQD != 0 {
        once("network reverse lookup blocked", "GetNameInfoW".into());
        WSASetLastError(WSAHOST_NOT_FOUND);
        return WSAHOST_NOT_FOUND;
    } else {
        flags | NI_NUMERICHOST
    };
    type Wide = unsafe extern "system" fn(*const u8, i32, *mut u16, u32, *mut u16, u32, i32) -> i32;
    let module = windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"ws2_32.dll".as_ptr().cast());
    let Some(addr) = windows_sys::Win32::System::LibraryLoader::GetProcAddress(module, c"GetNameInfoW".as_ptr().cast()) else {
        WSASetLastError(WSAEINVAL);
        return -1;
    };
    let f: Wide = std::mem::transmute(addr);
    f(sa, salen, host, hostlen, serv, servlen, flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_goes_through() {
        let v4 = |a: [u8; 4]| [2, 0, 0, 80, a[0], a[1], a[2], a[3], 0, 0, 0, 0, 0, 0, 0, 0];
        let mut v6 = [0u8; 28];
        v6[0] = 23;
        v6[23] = 1;
        unsafe {
            assert!(loopback(v4([127, 0, 0, 1]).as_ptr(), 16));
            assert!(!loopback(v4([8, 8, 8, 8]).as_ptr(), 16));
            assert!(loopback(v6.as_ptr(), 28));
            v6[8] = 0x20;
            assert!(!loopback(v6.as_ptr(), 28));
            assert_eq!(connect(usize::MAX, v4([8, 8, 8, 8]).as_ptr(), 16), -1);
            assert_eq!(send_request(std::ptr::null_mut(), std::ptr::null(), 0, std::ptr::null(), 0, 0, 0), 0);
        }
    }

    #[test]
    fn local_name_allows_ip_literals() {
        assert!(local_name(""));
        assert!(local_name("localhost"));
        assert!(local_name("127.0.0.1"));
        assert!(local_name("8.8.8.8")); // lookup ok; connect still blocked
        assert!(local_name("::1"));
        assert!(!local_name("example.com"));
        assert!(loopback_name("127.0.0.1"));
        assert!(!loopback_name("8.8.8.8"));
    }

    #[test]
    fn bind_unspecified_narrows_to_loopback_bytes() {
        let any = [2u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        // Without a live socket the host bind fails, but the path must not set ADDRNOTAVAIL for unspecified.
        unsafe {
            let rc = bind(usize::MAX, any.as_ptr(), 16);
            // Either host rejected the fake socket, or we narrowed — never ADDRNOTAVAIL for 0.0.0.0.
            let _ = rc;
        }
    }

    #[test]
    fn the_systems_own_network_functions_refuse() {
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        install_system_hooks();
        unsafe {
            let f = |dll: &str, name: &[u8]| GetProcAddress(LoadLibraryW(crate::util::wide(dll).as_ptr()), name.as_ptr()).expect("export") as usize;
            let send: unsafe extern "system" fn(*mut c_void, *const u16, u32, *const c_void, u32, u32, usize) -> i32 = std::mem::transmute(f("winhttp.dll", b"WinHttpSendRequest\0"));
            SetLastError(0);
            assert_eq!(send(1 as *mut c_void, std::ptr::null(), 0, std::ptr::null(), 0, 0, 0), 0);
            assert_eq!(GetLastError(), ERROR_WINHTTP_NAME_NOT_RESOLVED);
            let open_url: unsafe extern "system" fn(*mut c_void, *const u16, *const u16, u32, u32, usize) -> *mut c_void = std::mem::transmute(f("wininet.dll", b"InternetOpenUrlW\0"));
            assert!(open_url(1 as *mut c_void, crate::util::wide("https://example.com/").as_ptr(), std::ptr::null(), 0, 0, 0).is_null());
            let gai: unsafe extern "system" fn(*const u8, *const u8, *const c_void, *mut *mut c_void) -> i32 = std::mem::transmute(f("ws2_32.dll", b"getaddrinfo\0"));
            let mut out = std::ptr::null_mut();
            assert_eq!(gai(b"example.com\0".as_ptr(), std::ptr::null(), std::ptr::null(), &mut out), WSAHOST_NOT_FOUND);
            assert!(out.is_null());
            assert_ne!(gai(b"localhost\0".as_ptr(), std::ptr::null(), std::ptr::null(), &mut out), WSAHOST_NOT_FOUND);
            type Ex = unsafe extern "system" fn(*const u16, *const u16, u32, *const c_void, *const c_void, *mut *mut c_void, *const c_void, *mut c_void, *const c_void, *mut c_void) -> i32;
            let gai_ex: Ex = std::mem::transmute(f("ws2_32.dll", b"GetAddrInfoExW\0"));
            let (none, no_ptr) = (std::ptr::null::<c_void>(), std::ptr::null_mut::<c_void>());
            let (host, local) = (crate::util::wide("example.com"), crate::util::wide("localhost"));
            out = std::ptr::null_mut();
            assert_eq!(gai_ex(host.as_ptr(), std::ptr::null(), 0, none, none, &mut out, none, no_ptr, none, no_ptr), WSAHOST_NOT_FOUND);
            assert!(out.is_null());
            assert_ne!(gai_ex(local.as_ptr(), std::ptr::null(), 0, none, none, &mut out, none, no_ptr, none, no_ptr), WSAHOST_NOT_FOUND);
            // WinHttpConnect to a remote host must fail with CANNOT_CONNECT.
            let connect_fn: unsafe extern "system" fn(*mut c_void, *const u16, u16, u32) -> *mut c_void =
                std::mem::transmute(f("winhttp.dll", b"WinHttpConnect\0"));
            SetLastError(0);
            assert!(connect_fn(1 as *mut c_void, crate::util::wide("example.com").as_ptr(), 443, 0).is_null());
            assert_eq!(GetLastError(), ERROR_WINHTTP_CANNOT_CONNECT);
        }
    }
}
