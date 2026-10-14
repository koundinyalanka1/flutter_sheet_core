// The public Dart API over the Rust spreadsheet engine.

import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

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
  /// `ENCRYPTED`, `TOO_LARGE`, `BAD_HANDLE`, `PANIC` or `ERROR`.
  final String code;

  /// For `INVALID_ARGUMENT` from an edit, a sentence to show the person
  /// ("There's a problem with this formula...").
  final String message;

  @override
  String toString() => 'SheetCoreException($code): $message';
}

/// The order the locale writes dates in, for typed dates.
enum SheetDateOrder {
  dmy('dmy'),
  mdy('mdy'),
  ymd('ymd');

  const SheetDateOrder(this.code);
  final String code;
}

/// File formats a workbook can be saved in.
enum SheetFormat {
  xlsx('xlsx'),
  xlsm('xlsm'),
  ods('ods'),
  csv('csv'),
  tsv('tsv');

  const SheetFormat(this.code);
  final String code;

  static SheetFormat? fromCode(String? code) {
    for (final f in values) {
      if (f.code == code) return f;
    }
    return null;
  }

  /// The format for a file name's extension.
  static SheetFormat? forName(String name) {
    final dot = name.lastIndexOf('.');
    if (dot < 0) return null;
    return switch (name.substring(dot + 1).toLowerCase()) {
      'xlsx' || 'xltx' => SheetFormat.xlsx,
      'xlsm' || 'xltm' => SheetFormat.xlsm,
      'ods' || 'ots' => SheetFormat.ods,
      'csv' || 'txt' => SheetFormat.csv,
      'tsv' || 'tab' => SheetFormat.tsv,
      _ => null,
    };
  }
}

/// A rectangle of cells, 0-based and inclusive.
class CellRange {
  const CellRange(this.r0, this.c0, this.r1, this.c1);

  factory CellRange.fromJson(List<dynamic> j) =>
      CellRange(j[0] as int, j[1] as int, j[2] as int, j[3] as int);

  final int r0;
  final int c0;
  final int r1;
  final int c1;

  int get rows => r1 - r0 + 1;
  int get cols => c1 - c0 + 1;

  bool contains(int row, int col) =>
      row >= r0 && row <= r1 && col >= c0 && col <= c1;

  List<int> toJson() => [r0, c0, r1, c1];

  @override
  bool operator ==(Object other) =>
      other is CellRange &&
      other.r0 == r0 &&
      other.c0 == c0 &&
      other.r1 == r1 &&
      other.c1 == c1;

  @override
  int get hashCode => Object.hash(r0, c0, r1, c1);

  @override
  String toString() => r0 == r1 && c0 == c1
      ? cellName(r0, c0)
      : '${cellName(r0, c0)}:${cellName(r1, c1)}';
}

/// `A`, `B`, ... `AA` for a 0-based column.
String columnName(int col) {
  var n = col + 1;
  final out = StringBuffer();
  final chars = <int>[];
  while (n > 0) {
    final rem = (n - 1) % 26;
    chars.add(65 + rem);
    n = (n - 1) ~/ 26;
  }
  for (final c in chars.reversed) {
    out.writeCharCode(c);
  }
  return out.toString();
}

/// `A1`-style name of a 0-based cell.
String cellName(int row, int col) => '${columnName(col)}${row + 1}';

int? _color(Object? hex) {
  if (hex is! String || hex.length != 7) return null;
  final v = int.tryParse(hex.substring(1), radix: 16);
  return v == null ? null : 0xFF000000 | v;
}

/// One sheet of a workbook.
class SheetInfo {
  const SheetInfo({
    required this.name,
    required this.state,
    required this.used,
    required this.freeze,
    required this.showGrid,
    required this.defaultColWidth,
    required this.defaultRowHeight,
    required this.chart,
  });

  factory SheetInfo.fromJson(Map<String, dynamic> j) => SheetInfo(
    name: j['name'] as String,
    state: j['state'] as String? ?? 'visible',
    used: j['used'] == null ? null : CellRange.fromJson(j['used'] as List),
    freeze: j['freeze'] == null
        ? null
        : (
            row: (j['freeze'] as List)[0] as int,
            col: (j['freeze'] as List)[1] as int,
          ),
    showGrid: j['showGrid'] != false,
    defaultColWidth: (j['defaultColWidth'] as num?)?.toDouble() ?? 8.43,
    defaultRowHeight: (j['defaultRowHeight'] as num?)?.toDouble() ?? 15,
    chart: j['chart'] == true,
  );

  final String name;

  /// `visible`, `hidden` or `veryHidden`.
  final String state;

  /// The cells in use, or null for an empty sheet.
  final CellRange? used;

  /// The first cell below and right of frozen panes.
  final ({int row, int col})? freeze;
  final bool showGrid;

  /// In characters of the default font's digits (Excel's unit).
  final double defaultColWidth;

  /// In points.
  final double defaultRowHeight;

  /// A chart sheet, which has no cells.
  final bool chart;

  bool get visible => state == 'visible';
}

/// A defined name.
class SheetName {
  const SheetName(this.name, this.sheet, this.refersTo);
  final String name;
  final int? sheet;
  final String refersTo;
}

/// What the engine knows about an open workbook.
class WorkbookInfo {
  const WorkbookInfo({
    required this.format,
    required this.importedFrom,
    required this.writesBack,
    required this.saveFormat,
    required this.sheets,
    required this.active,
    required this.modified,
    required this.canUndo,
    required this.canRedo,
    required this.warnings,
    required this.circular,
    required this.names,
  });

  factory WorkbookInfo.fromJson(Map<String, dynamic> j) => WorkbookInfo(
    format: SheetFormat.fromCode(j['format'] as String?),
    importedFrom: j['importedFrom'] as String?,
    writesBack: j['writesBack'] as bool? ?? true,
    saveFormat:
        SheetFormat.fromCode(j['saveFormat'] as String?) ??
        SheetFormat.fromCode(j['format'] as String?) ??
        SheetFormat.xlsx,
    sheets: [
      for (final s in (j['sheets'] as List).cast<Map<String, dynamic>>())
        SheetInfo.fromJson(s),
    ],
    active: j['active'] as int? ?? 0,
    modified: j['modified'] == true,
    canUndo: j['canUndo'] == true,
    canRedo: j['canRedo'] == true,
    warnings: (j['warnings'] as List? ?? const []).cast<String>(),
    circular: [
      for (final c in (j['circular'] as List? ?? const []).cast<List>())
        (sheet: c[0] as int, row: c[1] as int, col: c[2] as int),
    ],
    names: [
      for (final n
          in (j['names'] as List? ?? const []).cast<Map<String, dynamic>>())
        SheetName(
          n['name'] as String,
          n['sheet'] as int?,
          n['refersTo'] as String? ?? '',
        ),
    ],
  );

  /// The format it was read from or last saved in; null for a new one.
  final SheetFormat? format;

  /// The format of the file it was opened from when that is not written
  /// back: `xls` (Excel 97-2003, WPS) or `fods` (a flat OpenDocument
  /// spreadsheet); null otherwise.
  final String? importedFrom;

  /// Whether saving writes the opened file back in its own format. When
  /// false, save to a new file whose extension is [saveFormat]'s code.
  final bool writesBack;

  /// The format saving writes: [format] when [writesBack]; otherwise
  /// [SheetFormat.xlsx] for `xls`, or [SheetFormat.ods] for `fods`.
  final SheetFormat saveFormat;
  final List<SheetInfo> sheets;
  final int active;

  /// Edits since the last save.
  final bool modified;
  final bool canUndo;
  final bool canRedo;

  /// Things the reader could not keep or understand.
  final List<String> warnings;

  /// Cells in circular references found by the last recalculation.
  final List<({int sheet, int row, int col})> circular;
  final List<SheetName> names;
}

/// A cell as the grid draws it.
class GridCell {
  const GridCell(
    this.row,
    this.col,
    this.text,
    this.kind,
    this.style,
    this.color,
    this.formula,
  );

  final int row;
  final int col;

  /// The value formatted with its number format.
  final String text;

  /// `n` number, `s` text, `b` boolean, `e` error, `` empty.
  final String kind;
  final int style;

  /// A colour the number format asks for (`[Red]`), as ARGB.
  final int? color;
  final bool formula;
}

/// The cells of a window of a sheet, with the merges, column widths and
/// row heights in it.
class CellWindow {
  const CellWindow(this.cells, this.merges, this.cols, this.rows);

  factory CellWindow.fromJson(Map<String, dynamic> j) => CellWindow(
    [
      for (final c in (j['cells'] as List).cast<List>())
        GridCell(
          c[0] as int,
          c[1] as int,
          c[2] as String,
          c[3] as String,
          c[4] as int,
          _color(c[5]),
          c[6] == true,
        ),
    ],
    [for (final m in (j['merges'] as List).cast<List>()) CellRange.fromJson(m)],
    {
      for (final c in (j['cols'] as List).cast<List>())
        c[0] as int: (width: (c[1] as num).toDouble(), hidden: c[2] == true),
    },
    {
      for (final r in (j['rows'] as List).cast<List>())
        r[0] as int: (height: (r[1] as num).toDouble(), hidden: r[2] == true),
    },
  );

  final List<GridCell> cells;
  final List<CellRange> merges;
  final Map<int, ({double width, bool hidden})> cols;
  final Map<int, ({double height, bool hidden})> rows;
}

/// Column widths, row heights and merges of a whole sheet.
class SheetLayout {
  const SheetLayout(this.cols, this.rows, this.merges);

  factory SheetLayout.fromJson(Map<String, dynamic> j) => SheetLayout(
    {
      for (final c in (j['cols'] as List).cast<List>())
        c[0] as int: (width: (c[1] as num).toDouble(), hidden: c[2] == true),
    },
    {
      for (final r in (j['rows'] as List).cast<List>())
        r[0] as int: (height: (r[1] as num).toDouble(), hidden: r[2] == true),
    },
    [for (final m in (j['merges'] as List).cast<List>()) CellRange.fromJson(m)],
  );

  final Map<int, ({double width, bool hidden})> cols;
  final Map<int, ({double height, bool hidden})> rows;
  final List<CellRange> merges;
}

/// One side of a cell's border.
class CellEdge {
  const CellEdge(this.style, this.color);
  final String style;
  final int color;
}

/// How a cell looks.
class CellStyleInfo {
  const CellStyleInfo({
    required this.id,
    required this.font,
    required this.size,
    required this.bold,
    required this.italic,
    required this.underline,
    required this.strike,
    required this.color,
    required this.fill,
    required this.hAlign,
    required this.vAlign,
    required this.wrap,
    required this.indent,
    required this.rotation,
    required this.numFmt,
    this.left,
    this.right,
    this.top,
    this.bottom,
  });

  factory CellStyleInfo.fromJson(Map<String, dynamic> j) {
    final b = j['border'] as Map<String, dynamic>? ?? const {};
    CellEdge? edge(Object? e) {
      if (e is! List) return null;
      return CellEdge(e[0] as String, _color(e[1]) ?? 0xFF000000);
    }

    return CellStyleInfo(
      id: j['id'] as int,
      font: j['font'] as String? ?? 'Calibri',
      size: (j['size'] as num?)?.toDouble() ?? 11,
      bold: j['bold'] == true,
      italic: j['italic'] == true,
      underline: j['underline'] as String? ?? 'none',
      strike: j['strike'] == true,
      color: _color(j['color']),
      fill: _color(j['fill']),
      hAlign: j['hAlign'] as String? ?? '',
      vAlign: j['vAlign'] as String? ?? '',
      wrap: j['wrap'] == true,
      indent: j['indent'] as int? ?? 0,
      rotation: j['rotation'] as int? ?? 0,
      numFmt: j['numFmt'] as String? ?? 'General',
      left: edge(b['l']),
      right: edge(b['r']),
      top: edge(b['t']),
      bottom: edge(b['b']),
    );
  }

  final int id;
  final String font;

  /// Points.
  final double size;
  final bool bold;
  final bool italic;

  /// `none`, `single`, `double`, ...
  final String underline;
  final bool strike;

  /// Text colour as ARGB; null for automatic (black).
  final int? color;

  /// Fill as ARGB; null for none.
  final int? fill;

  /// `` (general), `left`, `center`, `right`, `justify`, `fill`...
  final String hAlign;

  /// `` (bottom), `top`, `center`.
  final String vAlign;
  final bool wrap;
  final int indent;
  final int rotation;
  final String numFmt;
  final CellEdge? left;
  final CellEdge? right;
  final CellEdge? top;
  final CellEdge? bottom;
}

/// A cell for the formula bar.
class CellDetail {
  const CellDetail({
    required this.edit,
    required this.text,
    required this.kind,
    required this.style,
    required this.formula,
    required this.array,
    required this.numFmt,
  });

  factory CellDetail.fromJson(Map<String, dynamic> j) => CellDetail(
    edit: j['edit'] as String? ?? '',
    text: j['text'] as String? ?? '',
    kind: j['kind'] as String? ?? '',
    style: j['style'] as int? ?? 0,
    formula: j['formula'] == true,
    array: j['array'] == true,
    numFmt: j['numFmt'] as String? ?? 'General',
  );

  /// What to edit: `=SUM(A1:A3)`, or the value as it would be typed.
  final String edit;
  final String text;
  final String kind;
  final int style;
  final bool formula;

  /// Part of an array formula (edit the whole range only).
  final bool array;
  final String numFmt;
}

/// What an edit changed.
class SheetEditResult {
  const SheetEditResult({
    required this.changed,
    required this.all,
    required this.structure,
    required this.circular,
    required this.canUndo,
    required this.canRedo,
  });

  factory SheetEditResult.fromJson(Map<String, dynamic> j) => SheetEditResult(
    changed: [
      for (final c in (j['changed'] as List? ?? const []).cast<List>())
        (sheet: c[0] as int, row: c[1] as int, col: c[2] as int),
    ],
    all: j['all'] == true,
    structure: j['structure'] == true,
    circular: [
      for (final c in (j['circular'] as List? ?? const []).cast<List>())
        (sheet: c[0] as int, row: c[1] as int, col: c[2] as int),
    ],
    canUndo: j['canUndo'] == true,
    canRedo: j['canRedo'] == true,
  );

  /// Cells whose value or look changed.
  final List<({int sheet, int row, int col})> changed;

  /// Too many changed to list, or the structure changed: refresh all.
  final bool all;

  /// Rows, columns or sheets moved.
  final bool structure;
  final List<({int sheet, int row, int col})> circular;
  final bool canUndo;
  final bool canRedo;
}

T _withStrings<T>(List<String?> values, T Function(List<Pointer<Utf8>>) body) {
  final pointers = [
    for (final value in values) value == null ? nullptr : value.toNativeUtf8(),
  ];
  try {
    return body(pointers);
  } finally {
    for (final p in pointers) {
      if (p != nullptr) malloc.free(p);
    }
  }
}

SheetCoreException _lastError(SheetBindings b) =>
    SheetCoreException.fromNative(b.lastError().toDartString());

String _takeString(SheetBindings b, Pointer<Utf8> ptr) {
  if (ptr == nullptr) throw _lastError(b);
  try {
    return ptr.toDartString();
  } finally {
    b.freeString(ptr);
  }
}

int _checked(SheetBindings b, int result) {
  if (result < 0) throw _lastError(b);
  return result;
}

String _options(SheetDateOrder order, int? utcOffsetMinutes) => jsonEncode({
  'dateOrder': order.code,
  'utcOffsetMinutes': ?utcOffsetMinutes,
});

/// Entry points of the spreadsheet engine.
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

  /// Open a workbook on a background isolate. [name] supplies the file
  /// name when [path] has no extension.
  static Future<SheetBook> open(
    String path, {
    String? name,
    SheetDateOrder dateOrder = SheetDateOrder.dmy,
    int? utcOffsetMinutes,
  }) async {
    final options = _options(dateOrder, utcOffsetMinutes);
    final handle = await Isolate.run(() {
      final b = SheetBindings.instance;
      return _withStrings([path, name, options], (p) {
        final h = b.open(p[0], p[1], p[2]);
        if (h < 0) throw _lastError(b);
        return h;
      });
    });
    return SheetBook._(handle);
  }

  /// A new workbook with one empty sheet.
  static SheetBook create({
    SheetDateOrder dateOrder = SheetDateOrder.dmy,
    int? utcOffsetMinutes,
  }) {
    final b = SheetBindings.instance;
    final options = _options(dateOrder, utcOffsetMinutes);
    return SheetBook._(
      _withStrings([options], (p) => _checked(b, b.create(p[0]))),
    );
  }

  /// Every worksheet function the engine computes.
  static List<String> functionNames() {
    final b = SheetBindings.instance;
    return (jsonDecode(_takeString(b, b.functionNamesJson())) as List)
        .cast<String>();
  }

  /// How [value] looks in format [code].
  static ({String text, int? color}) formatPreview(
    double value,
    String code, {
    SheetDateOrder dateOrder = SheetDateOrder.dmy,
  }) {
    final b = SheetBindings.instance;
    final j =
        jsonDecode(
              _withStrings(
                [code],
                (p) => _takeString(
                  b,
                  b.formatPreview(value, p[0], dateOrder.index),
                ),
              ),
            )
            as Map<String, dynamic>;
    return (text: j['text'] as String, color: _color(j['color']));
  }
}

/// An open workbook. Call [close] when done with it.
class SheetBook {
  SheetBook._(this.handle);

  /// The engine's handle; usable from any isolate.
  final int handle;
  bool _closed = false;

  bool get isClosed => _closed;

  SheetBindings get _b {
    if (_closed) throw StateError('The workbook is closed.');
    return SheetBindings.instance;
  }

  Map<String, dynamic> _json(Pointer<Utf8> ptr) =>
      jsonDecode(_takeString(_b, ptr)) as Map<String, dynamic>;

  WorkbookInfo info() => WorkbookInfo.fromJson(_json(_b.infoJson(handle)));

  /// The cells of rows [r0]..[r1] and columns [c0]..[c1] of [sheet].
  CellWindow window(int sheet, int r0, int c0, int r1, int c1) =>
      CellWindow.fromJson(_json(_b.cellsJson(handle, sheet, r0, c0, r1, c1)));

  /// Column widths, row heights and merges of a whole sheet.
  SheetLayout layout(int sheet) =>
      SheetLayout.fromJson(_json(_b.layoutJson(handle, sheet)));

  /// Styles by id.
  Map<int, CellStyleInfo> styles(Iterable<int> ids) {
    final b = _b;
    final list =
        jsonDecode(
              _withStrings([
                jsonEncode(ids.toList()),
              ], (p) => _takeString(b, b.stylesJson(handle, p[0]))),
            )
            as List;
    return {
      for (final s in list.cast<Map<String, dynamic>>())
        s['id'] as int: CellStyleInfo.fromJson(s),
    };
  }

  /// One cell for the formula bar.
  CellDetail cell(int sheet, int row, int col) =>
      CellDetail.fromJson(_json(_b.cellJson(handle, sheet, row, col)));

  /// Apply an edit (see the engine's `sheet_apply` for operations). Throws
  /// [SheetCoreException] with code `INVALID_ARGUMENT` and a message for the
  /// person when the edit is refused.
  SheetEditResult apply(Map<String, Object?> op) {
    final b = _b;
    final json = _withStrings([
      jsonEncode(op),
    ], (p) => _takeString(b, b.apply(handle, p[0])));
    return SheetEditResult.fromJson(jsonDecode(json) as Map<String, dynamic>);
  }

  /// [apply] on a background isolate, for edits that touch many cells
  /// (sorting, inserting rows in a big sheet, pasting a large block).
  Future<SheetEditResult> applyAsync(Map<String, Object?> op) {
    _b;
    final handle = this.handle;
    final encoded = jsonEncode(op);
    return Isolate.run(() {
      final b = SheetBindings.instance;
      final json = _withStrings([
        encoded,
      ], (p) => _takeString(b, b.apply(handle, p[0])));
      return SheetEditResult.fromJson(jsonDecode(json) as Map<String, dynamic>);
    });
  }

  /// Type into a cell as a person would (`=SUM(A1:A3)`, `12%`, a date).
  SheetEditResult setCell(int sheet, int row, int col, String input) => apply({
    'op': 'set',
    'sheet': sheet,
    'row': row,
    'col': col,
    'input': input,
  });

  SheetEditResult? undo() {
    final s = _takeString(_b, _b.undo(handle));
    if (s == 'null') return null;
    return SheetEditResult.fromJson(jsonDecode(s) as Map<String, dynamic>);
  }

  SheetEditResult? redo() {
    final s = _takeString(_b, _b.redo(handle));
    if (s == 'null') return null;
    return SheetEditResult.fromJson(jsonDecode(s) as Map<String, dynamic>);
  }

  /// Recalculate every formula.
  SheetEditResult recalculate() =>
      SheetEditResult.fromJson(_json(_b.recalc(handle)));

  /// Save to [path] atomically on a background isolate; later saves build
  /// on it. Returns warnings to show (what a CSV file cannot hold).
  Future<List<String>> save(String path, SheetFormat format) =>
      _write(path, format, copy: false);

  /// Write a copy without changing what the workbook is saved as.
  Future<List<String>> writeCopy(String path, SheetFormat format) =>
      _write(path, format, copy: true);

  Future<List<String>> _write(
    String path,
    SheetFormat format, {
    required bool copy,
  }) {
    _b;
    final handle = this.handle;
    final code = format.code;
    return Isolate.run(() {
      final b = SheetBindings.instance;
      final json = _withStrings(
        [path, code],
        (p) => _takeString(
          b,
          copy ? b.writeCopy(handle, p[0], p[1]) : b.save(handle, p[0], p[1]),
        ),
      );
      final j = jsonDecode(json) as Map<String, dynamic>;
      return (j['warnings'] as List).cast<String>();
    });
  }

  /// Print to a PDF at [path] on a background isolate, with fonts from
  /// [fontDirs]: every visible sheet, or [sheets]. Returns the page count.
  Future<int> exportPdf(
    String path, {
    List<String> fontDirs = const [],
    List<int> sheets = const [],
    bool gridlines = true,
    String? title,
  }) {
    _b;
    final handle = this.handle;
    final options = jsonEncode({
      'fontDirs': fontDirs,
      'sheets': sheets,
      'gridlines': gridlines,
      'title': title,
    });
    return Isolate.run(() {
      final b = SheetBindings.instance;
      final json = _withStrings([
        path,
        options,
      ], (p) => _takeString(b, b.exportPdf(handle, p[0], p[1])));
      return (jsonDecode(json) as Map<String, dynamic>)['pages'] as int;
    });
  }

  /// Cells whose shown text contains [query]. [sheet] null searches all.
  List<({int sheet, int row, int col})> find(
    String query, {
    int? sheet,
    bool matchCase = false,
    bool wholeCell = false,
    bool inFormulas = false,
    int limit = 10000,
  }) {
    final b = _b;
    final flags =
        (matchCase ? 1 : 0) | (wholeCell ? 2 : 0) | (inFormulas ? 4 : 0);
    final json = _withStrings(
      [query],
      (p) =>
          _takeString(b, b.findJson(handle, p[0], flags, sheet ?? -1, limit)),
    );
    return [
      for (final f in (jsonDecode(json) as List).cast<List>())
        (sheet: f[0] as int, row: f[1] as int, col: f[2] as int),
    ];
  }

  /// Evaluate [formula] (without `=`) as if in a cell, without changing
  /// anything. Returns the value as text.
  String evaluate(int sheet, int row, int col, String formula) {
    final b = _b;
    final json = _withStrings([
      formula,
    ], (p) => _takeString(b, b.evaluateJson(handle, sheet, row, col, p[0])));
    return (jsonDecode(json) as Map<String, dynamic>)['text'] as String;
  }

  void setOptions({SheetDateOrder? dateOrder, int? utcOffsetMinutes}) {
    final b = _b;
    final json = jsonEncode({
      'dateOrder': ?dateOrder?.code,
      'utcOffsetMinutes': ?utcOffsetMinutes,
    });
    _withStrings([json], (p) => _checked(b, b.setOptions(handle, p[0])));
  }

  /// Free the workbook. Safe to call twice.
  void close() {
    if (_closed) return;
    _closed = true;
    SheetBindings.instance.close(handle);
  }
}
