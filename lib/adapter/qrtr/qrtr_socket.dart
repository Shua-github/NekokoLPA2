import 'dart:ffi';
import 'dart:io' show Platform;
import 'dart:typed_data';

import 'package:ffi/ffi.dart';

import 'qrtr_host.dart';

/// Where on the bus a datagram is going, or came from: a node and a port.
class QrtrAddress {
  const QrtrAddress(this.node, this.port);

  final int node;
  final int port;
}

/// One datagram, and where it came from.
class QrtrDatagram {
  const QrtrDatagram(this.from, this.data);

  final QrtrAddress from;
  final Uint8List data;
}

/// A datagram socket to the modem's bus.
///
/// This is the one thing a QMI transport needs from the link, so it is the one
/// thing a platform has to provide. On Linux the app opens the socket itself;
/// on Android it may not open one at all — SELinux refuses it the socket and
/// refuses it the use of one another process opened — so the socket stays in
/// the process Shizuku runs and this asks that process instead.
abstract class QrtrSocket {
  /// The address this socket sends from, which is where a lookup starts.
  Future<QrtrAddress> address();

  /// Send one datagram to an address on the bus.
  Future<void> send(QrtrAddress to, Uint8List datagram);

  /// Wait for one datagram, or give up after `timeoutMs`.
  Future<QrtrDatagram?> receive(int timeoutMs);

  /// Let the socket go.
  Future<void> close();
}

/// Open a socket to the bus, by whichever door this platform has.
Future<QrtrSocket> openQrtrSocket() async {
  if (Platform.isAndroid) {
    return QrtrHostSocket(await QrtrHost.openBus());
  }

  if (Platform.isLinux) {
    return QrtrFfiSocket.open();
  }

  throw UnsupportedError('QRTR is only reachable on Linux and Android');
}

/// `AF_QIPCRTR` as upstream defines it, then the number Qualcomm's own kernels
/// registered it under. Which one a device uses cannot be asked, only tried.
const List<int> _families = [43, 42];

/// Bytes one address on the bus takes.
const int _addressSize = 12;

/// Biggest datagram QRTR carries.
const int _maxDatagram = 65535;

const int _sockDgram = 2;
const int _sockCloexec = 0x80000;
const int _pollin = 0x001;

/// One address on the QRTR bus, as the kernel writes it.
final class _SockAddrQrtr extends Struct {
  @Uint16()
  external int family;

  @Uint32()
  external int node;

  @Uint32()
  external int port;
}

/// One entry of what `poll` watches, as the kernel writes it.
final class _PollFd extends Struct {
  @Int32()
  external int fd;

  @Int16()
  external int events;

  @Int16()
  external int revents;
}

typedef _SocketNative =
    Int32 Function(Int32 family, Int32 type, Int32 protocol);
typedef _SocketDart = int Function(int family, int type, int protocol);

typedef _CloseNative = Int32 Function(Int32 fd);
typedef _CloseDart = int Function(int fd);

typedef _GetsocknameNative =
    Int32 Function(
      Int32 fd,
      Pointer<_SockAddrQrtr> address,
      Pointer<Uint32> length,
    );
typedef _GetsocknameDart =
    int Function(
      int fd,
      Pointer<_SockAddrQrtr> address,
      Pointer<Uint32> length,
    );

typedef _SendtoNative =
    Int32 Function(
      Int32 fd,
      Pointer<Uint8> datagram,
      IntPtr length,
      Int32 flags,
      Pointer<_SockAddrQrtr> to,
      Uint32 toLength,
    );
typedef _SendtoDart =
    int Function(
      int fd,
      Pointer<Uint8> datagram,
      int length,
      int flags,
      Pointer<_SockAddrQrtr> to,
      int toLength,
    );

typedef _RecvfromNative =
    Int32 Function(
      Int32 fd,
      Pointer<Uint8> datagram,
      IntPtr length,
      Int32 flags,
      Pointer<_SockAddrQrtr> from,
      Pointer<Uint32> fromLength,
    );
typedef _RecvfromDart =
    int Function(
      int fd,
      Pointer<Uint8> datagram,
      int length,
      int flags,
      Pointer<_SockAddrQrtr> from,
      Pointer<Uint32> fromLength,
    );

typedef _PollNative =
    Int32 Function(Pointer<_PollFd> fds, Uint32 count, Int32 timeout);
typedef _PollDart = int Function(Pointer<_PollFd> fds, int count, int timeout);

/// The socket itself, for a process the bus lets in.
class QrtrFfiSocket implements QrtrSocket {
  QrtrFfiSocket._(this._fd, this._family, this._node)
    : _datagram = calloc<Uint8>(_maxDatagram),
      _from = calloc<_SockAddrQrtr>(1),
      _fromLength = calloc<Uint32>(1),
      _wait = calloc<_PollFd>(1) {
    _fromLength.value = _addressSize;
  }

  final int _fd;
  final int _family;
  final int _node;

  final Pointer<Uint8> _datagram;
  final Pointer<_SockAddrQrtr> _from;
  final Pointer<Uint32> _fromLength;
  final Pointer<_PollFd> _wait;

  bool _closed = false;

  static final DynamicLibrary _libc = DynamicLibrary.process();

  static final _socket = _libc.lookupFunction<_SocketNative, _SocketDart>(
    'socket',
  );
  static final _close = _libc.lookupFunction<_CloseNative, _CloseDart>('close');
  static final _getsockname = _libc
      .lookupFunction<_GetsocknameNative, _GetsocknameDart>('getsockname');
  static final _sendto = _libc.lookupFunction<_SendtoNative, _SendtoDart>(
    'sendto',
  );
  static final _recvfrom = _libc.lookupFunction<_RecvfromNative, _RecvfromDart>(
    'recvfrom',
  );
  static final _poll = _libc.lookupFunction<_PollNative, _PollDart>('poll');

  /// Open this process's socket, trying each family the bus may be on.
  static QrtrFfiSocket open() {
    for (final family in _families) {
      final fd = _socket(family, _sockDgram | _sockCloexec, 0);
      if (fd < 0) continue;

      final address = calloc<_SockAddrQrtr>(1);
      final length = calloc<Uint32>(1);
      length.value = _addressSize;

      // The kernel binds the socket on its first send, so its address is only
      // asked for to learn the node a lookup starts from.
      final named = _getsockname(fd, address, length);
      final node = named == 0 ? address.ref.node : 0;

      calloc.free(address);
      calloc.free(length);

      return QrtrFfiSocket._(fd, family, node);
    }

    throw StateError('this device has no QRTR address family');
  }

  @override
  Future<QrtrAddress> address() async => QrtrAddress(_node, 0);

  @override
  Future<void> send(QrtrAddress to, Uint8List datagram) async {
    final payload = malloc<Uint8>(datagram.length);
    payload.asTypedList(datagram.length).setAll(0, datagram);

    final address = calloc<_SockAddrQrtr>(1);
    address.ref
      ..family = _family
      ..node = to.node
      ..port = to.port;

    final written = _sendto(
      _fd,
      payload,
      datagram.length,
      0,
      address,
      _addressSize,
    );

    malloc.free(payload);
    calloc.free(address);

    if (written < 0) {
      throw StateError('could not send to node ${to.node} port ${to.port}');
    }
  }

  @override
  Future<QrtrDatagram?> receive(int timeoutMs) async {
    _wait.ref
      ..fd = _fd
      ..events = _pollin
      ..revents = 0;

    // `poll` re-checks the deadline, so a short wait that returns early is
    // simply asked again until the time is up.
    final until = DateTime.now().add(Duration(milliseconds: timeoutMs));

    while (true) {
      final ready = _poll(_wait, 1, timeoutMs);
      if (ready < 0) return null;
      if (ready > 0) break;
      if (DateTime.now().isAfter(until)) return null;
    }

    _fromLength.value = _addressSize;

    final read = _recvfrom(_fd, _datagram, _maxDatagram, 0, _from, _fromLength);
    if (read < 0) return null;

    return QrtrDatagram(
      QrtrAddress(_from.ref.node, _from.ref.port),
      Uint8List.fromList(_datagram.asTypedList(read)),
    );
  }

  @override
  Future<void> close() async {
    if (_closed) return;
    _closed = true;

    _close(_fd);
    calloc.free(_datagram);
    calloc.free(_from);
    calloc.free(_fromLength);
    calloc.free(_wait);
  }
}

/// The socket in the process Shizuku runs, reached a datagram at a time.
class QrtrHostSocket implements QrtrSocket {
  QrtrHostSocket(this._bus);

  final int _bus;

  @override
  Future<QrtrAddress> address() async =>
      QrtrAddress(await QrtrHost.node(_bus), 0);

  @override
  Future<void> send(QrtrAddress to, Uint8List datagram) =>
      QrtrHost.send(_bus, to.node, to.port, datagram);

  @override
  Future<QrtrDatagram?> receive(int timeoutMs) async {
    final packet = await QrtrHost.receive(_bus, timeoutMs);
    if (packet == null) return null;

    // The address it came from is in front of the message: node then port,
    // both 32 bit little endian.
    final header = ByteData.sublistView(packet);

    return QrtrDatagram(
      QrtrAddress(
        header.getUint32(0, Endian.little),
        header.getUint32(4, Endian.little),
      ),
      packet.sublist(8),
    );
  }

  @override
  Future<void> close() => QrtrHost.closeBus(_bus);
}
