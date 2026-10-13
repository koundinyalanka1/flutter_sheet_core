// Runs the Dart API against the real native library. Point
// SHEET_CORE_LIB_PATH at libsheet_ffi (see scripts/build_engines.sh env).

import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_sheet_core/flutter_sheet_core.dart';

void main() {
  final available = SheetCore.isAvailable;

  test('explains engine errors as a code and a message', () {
    final error = SheetCoreException.fromNative('UNSUPPORTED: not yet');
    expect(error.code, 'UNSUPPORTED');
    expect(error.message, 'not yet');
    expect(SheetCoreException.fromNative('plain').code, 'ERROR');
  });

  test('loads the native library and reports its version', () {
    expect(SheetCore.isAvailable, isTrue);
    expect(SheetCore.version, isNotEmpty);
  }, skip: !available);
}
