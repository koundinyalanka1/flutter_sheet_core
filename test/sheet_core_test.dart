// Runs the Dart API against the real native library. Point
// SHEET_CORE_LIB_PATH at libsheet_ffi (see scripts/build_engines.sh env).

import 'dart:io';

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

  test('names cells and columns', () {
    expect(columnName(0), 'A');
    expect(columnName(25), 'Z');
    expect(columnName(26), 'AA');
    expect(cellName(9, 27), 'AB10');
    expect(const CellRange(0, 0, 2, 1).toString(), 'A1:B3');
    expect(SheetFormat.forName('Book.XLSX'), SheetFormat.xlsx);
    expect(SheetFormat.forName('data.csv'), SheetFormat.csv);
  });

  test('loads the native library and reports its version', () {
    expect(SheetCore.isAvailable, isTrue);
    expect(SheetCore.version, isNotEmpty);
    expect(SheetCore.functionNames(), contains('XLOOKUP'));
    expect(SheetCore.formatPreview(1234.5, '#,##0.00').text, '1,234.50');
  }, skip: !available);

  test('edits, recalculates, undoes and saves a workbook', () async {
    final book = SheetCore.create();
    book.setCell(0, 0, 0, '10');
    book.setCell(0, 1, 0, '20');
    final r = book.setCell(0, 2, 0, '=SUM(A1:A2)');
    expect(r.changed.any((c) => c.row == 2 && c.col == 0), isTrue);
    expect(book.window(0, 0, 0, 5, 5).cells.last.text, '30');
    book.setCell(0, 0, 0, '15');
    expect(book.cell(0, 2, 0).text, '35');
    expect(book.cell(0, 2, 0).edit, '=SUM(A1:A2)');
    book.undo();
    expect(book.cell(0, 2, 0).text, '30');
    expect(
      () => book.setCell(0, 3, 0, '=SUM('),
      throwsA(
        isA<SheetCoreException>().having(
          (e) => e.code,
          'code',
          'INVALID_ARGUMENT',
        ),
      ),
    );
    book.apply({
      'op': 'format',
      'sheet': 0,
      'ranges': [
        [0, 0, 0, 0],
      ],
      'patch': {'bold': true},
    });
    final style = book.window(0, 0, 0, 0, 0).cells.single.style;
    expect(book.styles([style])[style]!.bold, isTrue);
    expect(book.evaluate(0, 9, 9, 'AVERAGE(A1:A2)'), '15');

    final dir = await Directory.systemTemp.createTemp('sheet_core_test');
    final path = '${dir.path}/book.xlsx';
    expect(await book.save(path, SheetFormat.xlsx), isEmpty);
    expect(book.info().modified, isFalse);
    book.close();

    final again = await SheetCore.open(path);
    final info = again.info();
    expect(info.format, SheetFormat.xlsx);
    expect(info.sheets.single.used, const CellRange(0, 0, 2, 0));
    expect(again.find('30').single.row, 2);
    final warnings = await again.writeCopy(
      '${dir.path}/book.csv',
      SheetFormat.csv,
    );
    expect(warnings, hasLength(1));
    expect(
      File('${dir.path}/book.csv').readAsStringSync(),
      '10\r\n20\r\n30\r\n',
    );
    again.close();
    await dir.delete(recursive: true);
  }, skip: !available);

  test('opens a flat OpenDocument spreadsheet and saves it as ODS', () async {
    final dir = await Directory.systemTemp.createTemp('sheet_core_flat');
    final flat = File('${dir.path}/bills.fods');
    await flat.writeAsString(
      '<?xml version="1.0" encoding="UTF-8"?>'
      '<office:document '
      'xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" '
      'xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" '
      'xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" '
      'office:version="1.3" '
      'office:mimetype="application/vnd.oasis.opendocument.spreadsheet">'
      '<office:body><office:spreadsheet><table:table table:name="Bills">'
      '<table:table-row><table:table-cell office:value-type="float" '
      'office:value="12"><text:p>12</text:p></table:table-cell>'
      '</table:table-row></table:table></office:spreadsheet></office:body>'
      '</office:document>',
    );
    final book = await SheetCore.open(flat.path);
    final info = book.info();
    expect(info.importedFrom, 'fods');
    expect(info.writesBack, isFalse);
    expect(info.saveFormat, SheetFormat.ods);
    expect(info.format, SheetFormat.ods);
    expect(info.warnings.first, contains('(.ods)'));
    expect(book.cell(0, 0, 0).text, '12');
    book.setCell(0, 0, 1, '=A1*2');

    final path = '${dir.path}/bills.${info.saveFormat.code}';
    expect(await book.save(path, info.saveFormat), isEmpty);
    book.close();
    final again = await SheetCore.open(path);
    expect(again.info().importedFrom, isNull);
    expect(again.info().writesBack, isTrue);
    expect(again.cell(0, 0, 1).text, '24');
    again.close();
    await dir.delete(recursive: true);
  }, skip: !available);

  test('prints a workbook to PDF', () async {
    final book = SheetCore.create();
    book.setCell(0, 0, 0, 'Item');
    book.setCell(0, 1, 0, 'Rice');
    final dir = await Directory.systemTemp.createTemp('sheet_pdf');
    final pages = await book.exportPdf(
      '${dir.path}/book.pdf',
      fontDirs: ['/usr/share/fonts'],
    );
    expect(pages, 1);
    expect(
      String.fromCharCodes(
        File('${dir.path}/book.pdf').readAsBytesSync().take(5),
      ),
      '%PDF-',
    );
    book.close();
    await dir.delete(recursive: true);
  }, skip: !available);
}
