//! The socket on the modem's bus, for the process Shizuku runs.
//!
//! An Android app may not open one — SELinux refuses it the socket and refuses
//! it the use of one another process opened — so the socket lives in the
//! process that is allowed to have it, and the app's transport asks that
//! process to send and to wait. The asking crosses Binder, and Binder can no
//! more name an address on the bus than Java can: an address there is a node
//! and a port, not an `InetAddress`. What that process therefore calls is this
//! module, which is where the socket really is.
//!
//! The entry points are the ones `QrtrUserService` declares: a descriptor, the
//! address it is bound to, one datagram in or out, and closing it again.

use core::mem::size_of;
use core::time::Duration;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd};
use std::time::Instant;
use std::vec::Vec;

use jni::objects::{JByteArray, JObject};
use jni::sys::{jbyteArray, jint};
use jni::JNIEnv;

/// `AF_QIPCRTR` as upstream defines it, then the number Qualcomm's own kernels
/// registered it under. Which one a device uses cannot be asked, only tried.
const FAMILIES: [libc::c_int; 2] = [43, 42];

/// Bytes a datagram carries in front of the message on its way back to the
/// app: the node and the port it came from.
const ADDRESS_LEN: usize = 8;

/// Biggest datagram QRTR carries.
const MAX_DATAGRAM: usize = 65_535;

/// One address on the QRTR bus, as the kernel writes it.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct SockAddrQrtr {
    sq_family: libc::sa_family_t,
    sq_node: u32,
    sq_port: u32,
}

/// Open a socket on the bus, answering the descriptor or a negative errno.
fn open() -> i32 {
    for family in FAMILIES {
        let socket = unsafe {
            libc::socket(
                family,
                libc::SOCK_DGRAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                0,
            )
        };

        if socket >= 0 {
            return socket;
        }
    }

    -io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EAFNOSUPPORT)
}

/// The address family a socket is on, which every address sent on it has to
/// name as well.
fn family(fd: i32) -> io::Result<libc::sa_family_t> {
    let mut address = SockAddrQrtr::default();
    let mut length = size_of::<SockAddrQrtr>() as libc::socklen_t;

    let named = unsafe {
        libc::getsockname(
            fd,
            (&mut address as *mut SockAddrQrtr).cast(),
            &mut length,
        )
    };

    if named < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(address.sq_family)
}

/// The node a socket is bound to.
fn node(fd: i32) -> io::Result<u32> {
    let mut address = SockAddrQrtr::default();
    let mut length = size_of::<SockAddrQrtr>() as libc::socklen_t;

    let named = unsafe {
        libc::getsockname(
            fd,
            (&mut address as *mut SockAddrQrtr).cast(),
            &mut length,
        )
    };

    if named < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(address.sq_node)
}

/// Send one datagram to an address on the bus.
fn send(fd: i32, node: u32, port: u32, datagram: &[u8]) -> io::Result<()> {
    let to = SockAddrQrtr {
        sq_family: family(fd)?,
        sq_node: node,
        sq_port: port,
    };

    let written = unsafe {
        libc::sendto(
            fd,
            datagram.as_ptr().cast(),
            datagram.len(),
            0,
            (&raw const to).cast(),
            size_of::<SockAddrQrtr>() as libc::socklen_t,
        )
    };

    if written < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

/// Wait for one datagram, with where it came from; `None` when none arrives in
/// time.
fn receive(fd: i32, timeout: Duration) -> io::Result<Option<(u32, u32, Vec<u8>)>> {
    let deadline = Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }

        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };

        let milliseconds = remaining.as_millis().min(libc::c_int::MAX as u128) as libc::c_int;
        let ready = unsafe { libc::poll(&mut poll, 1, milliseconds.max(1)) };

        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }

            return Err(error);
        }

        if ready == 0 {
            continue;
        }

        let mut datagram = std::vec![0u8; MAX_DATAGRAM];
        let mut from = SockAddrQrtr::default();
        let mut length = size_of::<SockAddrQrtr>() as libc::socklen_t;

        let read = unsafe {
            libc::recvfrom(
                fd,
                datagram.as_mut_ptr().cast(),
                datagram.len(),
                0,
                (&mut from as *mut SockAddrQrtr).cast(),
                &mut length,
            )
        };

        if read < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted || error.kind() == io::ErrorKind::WouldBlock
            {
                continue;
            }

            return Err(error);
        }

        datagram.truncate(read as usize);

        return Ok(Some((from.sq_node, from.sq_port, datagram)));
    }
}

/// Close a socket this process opened.
fn close(fd: i32) {
    // SAFETY: the descriptor came from `open` above and is not used again.
    drop(unsafe { OwnedFd::from_raw_fd(fd) });
}

/// Throw a Java exception, so a failing call is not a silent zero.
fn fail(env: &mut JNIEnv, error: impl core::fmt::Display) {
    let _ = env.throw_new("java/lang/IllegalStateException", error.to_string());
}

#[no_mangle]
pub extern "system" fn Java_ee_nekoko_nlpa2_QrtrUserService_nativeOpenBus(
    mut env: JNIEnv,
    _this: JObject,
) -> jint {
    let fd = open();

    if fd < 0 {
        fail(&mut env, format!("no QRTR socket: errno {}", -fd));
    }

    fd
}

#[no_mangle]
pub extern "system" fn Java_ee_nekoko_nlpa2_QrtrUserService_nativeBusNode(
    mut env: JNIEnv,
    _this: JObject,
    fd: jint,
) -> jint {
    match node(fd) {
        Ok(node) => node as jint,
        Err(error) => {
            fail(&mut env, error);
            -1
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_ee_nekoko_nlpa2_QrtrUserService_nativeSend(
    mut env: JNIEnv,
    _this: JObject,
    fd: jint,
    node: jint,
    port: jint,
    data: JByteArray,
) -> jint {
    let datagram = match env.convert_byte_array(&data) {
        Ok(datagram) => datagram,
        Err(error) => {
            fail(&mut env, error);
            return -1;
        }
    };

    match send(fd, node as u32, port as u32, &datagram) {
        Ok(()) => 0,
        Err(error) => {
            fail(&mut env, error);
            -1
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_ee_nekoko_nlpa2_QrtrUserService_nativeReceive(
    mut env: JNIEnv,
    _this: JObject,
    fd: jint,
    timeout_ms: jint,
) -> jbyteArray {
    let timeout = Duration::from_millis(u64::try_from(timeout_ms.max(0)).unwrap_or(0));

    let arrived = match receive(fd, timeout) {
        Ok(arrived) => arrived,
        Err(error) => {
            fail(&mut env, error);
            return std::ptr::null_mut();
        }
    };

    let Some((node, port, datagram)) = arrived else {
        // Nothing arrived in time, which the caller is told by getting null
        // rather than an empty datagram: a datagram can be empty.
        return std::ptr::null_mut();
    };

    let mut framed = Vec::with_capacity(ADDRESS_LEN + datagram.len());
    framed.extend_from_slice(&node.to_le_bytes());
    framed.extend_from_slice(&port.to_le_bytes());
    framed.extend_from_slice(&datagram);

    match env.byte_array_from_slice(&framed) {
        Ok(array) => array.into_raw(),
        Err(error) => {
            fail(&mut env, error);
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_ee_nekoko_nlpa2_QrtrUserService_nativeCloseBus(
    _env: JNIEnv,
    _this: JObject,
    fd: jint,
) {
    close(fd);
}
