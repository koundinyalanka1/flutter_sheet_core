# The Rust spreadsheet engine as a universal dylib. Build it with
# `bash scripts/build_engines.sh macos` from the app repository.
Pod::Spec.new do |s|
  s.name             = 'flutter_sheet_core'
  s.version          = '0.1.0'
  s.summary          = "DocHelper's spreadsheet engine."
  s.homepage         = 'https://github.com/koundinyalanka1/pdfhelper'
  s.license          = { :type => 'Proprietary' }
  s.author           = { 'Your Mate Apps' => 'yourmateapps@users.noreply.github.com' }
  s.source           = { :path => '.' }
  s.dependency 'FlutterMacOS'
  s.vendored_libraries = 'Frameworks/libsheet_ffi.dylib'
  s.platform = :osx, '10.14'
  s.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES' }
end
