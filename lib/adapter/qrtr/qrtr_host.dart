import 'package:flutter/services.dart';

import '../../utils/platform_adapter.dart';

/// Where the app reaches the modem's bus, through the process Shizuku runs.
///
/// An app may not open a socket on the bus, and may not use one another
/// process opened, so the socket stays in that process and what crosses to the
/// app is the traffic itself: a datagram to send, or a wait for one to arrive.
/// An address on the bus is a node and a port rather than anything Java can
/// name, which is why these calls end in the app's own native code.
class QrtrHost {
  static const MethodChannel _channel = MethodChannel('nlpa2/qrtr');

  /// Whether this device is likely to have a QRTR bus.
  ///
  /// A QRTR bus cannot be enumerated, and opening its socket takes Shizuku, so
  /// the sign to go on is the SoC: on a Qualcomm device the bus is there even
  /// when nothing is plugged into it.
  static Future<bool> isSupported() async {
    if (!PlatformX.isAndroid) return false;

    return await _channel.invokeMethod<bool>('isSupported') ?? false;
  }

  /// Whether Shizuku is installed, running, and new enough for user services.
  static Future<bool> isAvailable() async {
    if (!PlatformX.isAndroid) return false;

    return await _channel.invokeMethod<bool>('isAvailable') ?? false;
  }

  /// Whether the user has already let this app use Shizuku.
  static Future<bool> hasPermission() async {
    if (!PlatformX.isAndroid) return false;

    return await _channel.invokeMethod<bool>('hasPermission') ?? false;
  }

  /// Ask the user for that permission, answering whether it was granted.
  static Future<bool> requestPermission() async {
    if (!PlatformX.isAndroid) return false;

    return await _channel.invokeMethod<bool>('requestPermission') ?? false;
  }

  /// Open a socket on the bus, answering the descriptor it has there.
  static Future<int> openBus() async {
    final fd = await _channel.invokeMethod<int>('openBus');
    if (fd == null || fd < 0) {
      throw StateError('Could not open a socket on the modem bus');
    }

    return fd;
  }

  /// The node that socket is bound to, which is where a lookup starts.
  static Future<int> node(int bus) async {
    return await _channel.invokeMethod<int>('busNode', {'fd': bus}) ?? 0;
  }

  /// Send one datagram to an address on the bus.
  static Future<void> send(int bus, int node, int port, Uint8List data) async {
    await _channel.invokeMethod<void>('send', {
      'fd': bus,
      'node': node,
      'port': port,
      'data': data,
    });
  }

  /// Wait for one datagram, or give up after `timeoutMs`.
  ///
  /// The address it came from is in front of the message: node then port, both
  /// 32 bit little endian. Nothing arriving in time is answered with null.
  static Future<Uint8List?> receive(int bus, int timeoutMs) async {
    return await _channel.invokeMethod<Uint8List>('receive', {
      'fd': bus,
      'timeoutMs': timeoutMs,
    });
  }

  /// Let the socket in that process go.
  static Future<void> closeBus(int bus) async {
    if (!PlatformX.isAndroid) return;

    await _channel.invokeMethod<void>('closeBus', {'fd': bus});
  }
}
