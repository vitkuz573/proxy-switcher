use std::io;
use std::net::SocketAddr;
use std::os::unix::io::{AsRawFd, RawFd};
use tokio::net::{TcpSocket, TcpStream};

/// Firewall mark used for the engine's own traffic.
///
/// The daemon creates a TUN device and can point the default route at it. If
/// the daemon's own connections were captured by that device they would be
/// routed through a proxy — including the connection to the proxy under test.
/// That is a loop, and when the proxy is dead the engine loses the ability to
/// scrape, to check, or to reach anything at all: the outage the proxy was
/// supposed to solve becomes the outage.
///
/// Marking the engine's own sockets makes them bypass the TUN's routing table
/// unconditionally, whatever the routing configuration says.
pub const ENGINE_MARK: u32 = 0x5053;

/// Routing table the marked traffic uses.
pub const ENGINE_TABLE: u32 = 0x5053;

/// Apply `SO_MARK` to a socket.
///
/// Linux-specific: the mark is what policy routing reads to decide which
/// table a packet belongs to. It has to be set before the connection is made,
/// which is why the socket is built by hand.
fn apply_mark(fd: RawFd, mark: u32) -> io::Result<()> {
    let value = mark as libc::c_int;
    // SAFETY: `fd` is a valid open socket for the duration of the call, and
    // `value` is a correctly sized `c_int` for SO_MARK.
    let result = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_MARK,
            &value as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };

    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Open a TCP connection, optionally marked to bypass the TUN.
///
/// `tokio::net::TcpSocket` is used rather than `TcpStream::connect` because the
/// mark has to be set before the connection is made, and it exposes the file
/// descriptor. It also performs the non-blocking connect correctly, which a
/// hand-rolled socket does not.
pub async fn connect_tcp(addr: SocketAddr, mark: Option<u32>) -> io::Result<TcpStream> {
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    socket.set_reuseaddr(true)?;

    if let Some(mark) = mark {
        apply_mark(socket.as_raw_fd(), mark)?;
    }

    socket.connect(addr).await
}

/// Resolve a host and port into socket addresses.
pub async fn resolve(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    tokio::net::lookup_host((host, port))
        .await
        .map(|it| it.collect())
}

/// Connect to the first address that accepts.
pub async fn connect_first(addrs: &[SocketAddr], mark: Option<u32>) -> io::Result<TcpStream> {
    let mut last: Option<io::Error> = None;

    for addr in addrs {
        match connect_tcp(*addr, mark).await {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }

    Err(last
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no addresses to connect to")))
}
