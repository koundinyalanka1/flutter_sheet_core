// Raw dart:ffi bindings for the Rust sheet_ffi library.
//
// Strings are UTF-8 and NUL-terminated. Strings the library returns are
// freed with [SheetBindings.freeString]; `sheet_last_error` and
// `sheet_core_version` return borrowed pointers that must not be freed.
//
// This file is internal to the package (not exported), so its fields may use
// the private native signature typedefs.
// ignore_for_file: library_private_types_in_public_api

import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

typedef _StrC = Pointer<Utf8> Function();
typedef _FreeStringC = Void Function(Pointer<Utf8>);
typedef _FreeStringDart = void Function(Pointer<Utf8>);

/// The sheet library, resolved once per isolate.
class SheetBindings {
  SheetBindings._(DynamicLibrary lib)
    : lastError = lib.lookupFunction<_StrC, _StrC>('sheet_last_error'),
      freeString = lib.lookupFunction<_FreeStringC, _FreeStringDart>(
        'sheet_free_string',
      ),
      version = lib.lookupFunction<_StrC, _StrC>('sheet_core_version');

  /// The thread's last error; borrowed, do not free.
  final _StrC lastError;

  /// Frees a string returned by this library.
  final _FreeStringDart freeString;

  /// Static library version; borrowed, do not free.
  final _StrC version;

  static SheetBindings? _instance;

  /// The bindings for this isolate. Throws [ArgumentError] when the native
  /// library cannot be loaded.
  ///
  /// `SHEET_CORE_LIB_PATH` overrides the library location (host tests).
  static SheetBindings get instance => _instance ??= SheetBindings._(_open());

  static DynamicLibrary _open() {
    final override = Platform.environment['SHEET_CORE_LIB_PATH'];
    if (override != null && override.isNotEmpty) {
      return DynamicLibrary.open(override);
    }
    if (Platform.isAndroid || Platform.isLinux) {
      return DynamicLibrary.open('libsheet_ffi.so');
    }
    if (Platform.isMacOS || Platform.isIOS) {
      // CocoaPods links the vendored library into the app.
      final process = DynamicLibrary.process();
      if (process.providesSymbol('sheet_last_error')) return process;
      return DynamicLibrary.open('libsheet_ffi.dylib');
    }
    if (Platform.isWindows) {
      return DynamicLibrary.open('sheet_ffi.dll');
    }
    return DynamicLibrary.process();
  }
}
