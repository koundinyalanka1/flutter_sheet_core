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
typedef _OpenC = Int64 Function(Pointer<Utf8>, Pointer<Utf8>, Pointer<Utf8>);
typedef _OpenDart = int Function(Pointer<Utf8>, Pointer<Utf8>, Pointer<Utf8>);
typedef _NewC = Int64 Function(Pointer<Utf8>);
typedef _NewDart = int Function(Pointer<Utf8>);
typedef _HandleC = Int64 Function(Int64);
typedef _HandleDart = int Function(int);
typedef _HandleStrC = Pointer<Utf8> Function(Int64);
typedef _HandleStrDart = Pointer<Utf8> Function(int);
typedef _CellsC =
    Pointer<Utf8> Function(Int64, Int64, Int64, Int64, Int64, Int64);
typedef _CellsDart = Pointer<Utf8> Function(int, int, int, int, int, int);
typedef _SheetStrC = Pointer<Utf8> Function(Int64, Int64);
typedef _SheetStrDart = Pointer<Utf8> Function(int, int);
typedef _HandleArgC = Pointer<Utf8> Function(Int64, Pointer<Utf8>);
typedef _HandleArgDart = Pointer<Utf8> Function(int, Pointer<Utf8>);
typedef _CellC = Pointer<Utf8> Function(Int64, Int64, Int64, Int64);
typedef _CellDart = Pointer<Utf8> Function(int, int, int, int);
typedef _SaveC = Pointer<Utf8> Function(Int64, Pointer<Utf8>, Pointer<Utf8>);
typedef _SaveDart = Pointer<Utf8> Function(int, Pointer<Utf8>, Pointer<Utf8>);
typedef _FindC =
    Pointer<Utf8> Function(Int64, Pointer<Utf8>, Int32, Int64, Int64);
typedef _FindDart = Pointer<Utf8> Function(int, Pointer<Utf8>, int, int, int);
typedef _EvalC =
    Pointer<Utf8> Function(Int64, Int64, Int64, Int64, Pointer<Utf8>);
typedef _EvalDart = Pointer<Utf8> Function(int, int, int, int, Pointer<Utf8>);
typedef _SetOptionsC = Int64 Function(Int64, Pointer<Utf8>);
typedef _SetOptionsDart = int Function(int, Pointer<Utf8>);
typedef _PreviewC = Pointer<Utf8> Function(Double, Pointer<Utf8>, Int32);
typedef _PreviewDart = Pointer<Utf8> Function(double, Pointer<Utf8>, int);

/// The sheet library, resolved once per isolate.
class SheetBindings {
  SheetBindings._(DynamicLibrary lib)
    : lastError = lib.lookupFunction<_StrC, _StrC>('sheet_last_error'),
      freeString = lib.lookupFunction<_FreeStringC, _FreeStringDart>(
        'sheet_free_string',
      ),
      version = lib.lookupFunction<_StrC, _StrC>('sheet_core_version'),
      open = lib.lookupFunction<_OpenC, _OpenDart>('sheet_open'),
      create = lib.lookupFunction<_NewC, _NewDart>('sheet_new'),
      close = lib.lookupFunction<_HandleC, _HandleDart>('sheet_close'),
      infoJson = lib.lookupFunction<_HandleStrC, _HandleStrDart>(
        'sheet_info_json',
      ),
      cellsJson = lib.lookupFunction<_CellsC, _CellsDart>('sheet_cells_json'),
      layoutJson = lib.lookupFunction<_SheetStrC, _SheetStrDart>(
        'sheet_layout_json',
      ),
      stylesJson = lib.lookupFunction<_HandleArgC, _HandleArgDart>(
        'sheet_styles_json',
      ),
      cellJson = lib.lookupFunction<_CellC, _CellDart>('sheet_cell_json'),
      apply = lib.lookupFunction<_HandleArgC, _HandleArgDart>('sheet_apply'),
      undo = lib.lookupFunction<_HandleStrC, _HandleStrDart>('sheet_undo'),
      redo = lib.lookupFunction<_HandleStrC, _HandleStrDart>('sheet_redo'),
      save = lib.lookupFunction<_SaveC, _SaveDart>('sheet_save'),
      writeCopy = lib.lookupFunction<_SaveC, _SaveDart>('sheet_write_copy'),
      exportPdf = lib.lookupFunction<_SaveC, _SaveDart>('sheet_export_pdf'),
      findJson = lib.lookupFunction<_FindC, _FindDart>('sheet_find_json'),
      evaluateJson = lib.lookupFunction<_EvalC, _EvalDart>(
        'sheet_evaluate_json',
      ),
      recalc = lib.lookupFunction<_HandleStrC, _HandleStrDart>('sheet_recalc'),
      setOptions = lib.lookupFunction<_SetOptionsC, _SetOptionsDart>(
        'sheet_set_options',
      ),
      formatPreview = lib.lookupFunction<_PreviewC, _PreviewDart>(
        'sheet_format_preview',
      ),
      functionNamesJson = lib.lookupFunction<_StrC, _StrC>(
        'sheet_function_names_json',
      );

  /// The thread's last error; borrowed, do not free.
  final _StrC lastError;

  /// Frees a string returned by this library.
  final _FreeStringDart freeString;

  /// Static library version; borrowed, do not free.
  final _StrC version;
  final _OpenDart open;
  final _NewDart create;
  final _HandleDart close;
  final _HandleStrDart infoJson;
  final _CellsDart cellsJson;
  final _SheetStrDart layoutJson;
  final _HandleArgDart stylesJson;
  final _CellDart cellJson;
  final _HandleArgDart apply;
  final _HandleStrDart undo;
  final _HandleStrDart redo;
  final _SaveDart save;
  final _SaveDart writeCopy;
  final _SaveDart exportPdf;
  final _FindDart findJson;
  final _EvalDart evaluateJson;
  final _HandleStrDart recalc;
  final _SetOptionsDart setOptions;
  final _PreviewDart formatPreview;
  final _StrC functionNamesJson;

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
