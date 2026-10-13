// The public Dart API over the Rust spreadsheet engine.

import 'package:ffi/ffi.dart';

import 'bindings.dart';

/// A failure reported by the spreadsheet engine.
class SheetCoreException implements Exception {
  SheetCoreException(this.code, this.message);

  /// Parse the engine's `"CODE: message"`.
  factory SheetCoreException.fromNative(String raw) {
    final colon = raw.indexOf(': ');
    if (colon <= 0) return SheetCoreException('ERROR', raw);
    return SheetCoreException(
      raw.substring(0, colon),
      raw.substring(colon + 2),
    );
  }

  /// Stable code: `IO`, `INVALID_ARGUMENT`, `UNSUPPORTED`, `CORRUPT`,
  /// `BAD_HANDLE`, `PANIC` or `ERROR`.
  final String code;
  final String message;

  @override
  String toString() => 'SheetCoreException($code): $message';
}

/// Entry points of the spreadsheet engine.
///
/// The workbook surface (open a workbook, read and write cells, recalculate,
/// save) arrives with the engine's FFI layer; for now the API reports the
/// native library and lets callers check whether it can be loaded.
class SheetCore {
  SheetCore._();

  /// Whether the native library could be loaded.
  static bool get isAvailable {
    try {
      SheetBindings.instance;
      return true;
    } catch (_) {
      return false;
    }
  }

  /// The native library's version, e.g. `0.1.0`.
  static String get version => SheetBindings.instance.version().toDartString();
}
