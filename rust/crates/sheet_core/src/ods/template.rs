//! The parts of a new, empty OpenDocument spreadsheet.

use doc_zip::{Compression, ZipWriter};

pub(super) const MIMETYPE: &str = "application/vnd.oasis.opendocument.spreadsheet";

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="settings.xml" manifest:media-type="text/xml"/></manifest:manifest>"#;

const NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" xmlns:number="urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2" xmlns:calcext="urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0" xmlns:loext="urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0""#;

pub(super) fn content() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content {NS} office:version="1.3"><office:font-face-decls><style:font-face style:name="Calibri" svg:font-family="Calibri" style:font-family-generic="swiss"/></office:font-face-decls><office:automatic-styles></office:automatic-styles><office:body><office:spreadsheet></office:spreadsheet></office:body></office:document-content>"#
    )
}

fn styles() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-styles {NS} office:version="1.3"><office:font-face-decls><style:font-face style:name="Calibri" svg:font-family="Calibri" style:font-family-generic="swiss"/></office:font-face-decls><office:styles><style:default-style style:family="table-cell"><style:table-cell-properties style:decimal-places="2"/><style:paragraph-properties style:tab-stop-distance="0.5in"/><style:text-properties style:font-name="Calibri" fo:font-size="11pt" fo:language="en" fo:country="IN"/></style:default-style><number:number-style style:name="N0"><number:number number:min-integer-digits="1"/></number:number-style><style:style style:name="Default" style:family="table-cell"/></office:styles><office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties style:writing-mode="lr-tb"/></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Default" style:page-layout-name="pm1"/></office:master-styles></office:document-styles>"#
    )
}

fn meta() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-meta {NS} office:version="1.3"><office:meta><meta:generator>DocHelper</meta:generator></office:meta></office:document-meta>"#
    )
}

pub(super) const SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-settings xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0" office:version="1.3"><office:settings><config:config-item-set config:name="ooo:view-settings"><config:config-item-map-indexed config:name="Views"><config:config-item-map-entry><config:config-item config:name="ViewId" config:type="string">view1</config:config-item><config:config-item-map-named config:name="Tables"></config:config-item-map-named></config:config-item-map-entry></config:config-item-map-indexed></config:config-item-set></office:settings></office:document-settings>"#;

/// The bytes of an empty spreadsheet package.
pub(super) fn bytes() -> Vec<u8> {
    let mut w = ZipWriter::new();
    w.add("mimetype", MIMETYPE.as_bytes(), Compression::Stored).expect("unique");
    w.add("META-INF/manifest.xml", MANIFEST.as_bytes(), Compression::DEFAULT).expect("unique");
    w.add("styles.xml", styles().as_bytes(), Compression::DEFAULT).expect("unique");
    w.add("meta.xml", meta().as_bytes(), Compression::DEFAULT).expect("unique");
    w.add("settings.xml", SETTINGS.as_bytes(), Compression::DEFAULT).expect("unique");
    w.add("content.xml", content().as_bytes(), Compression::DEFAULT).expect("unique");
    w.finish()
}
