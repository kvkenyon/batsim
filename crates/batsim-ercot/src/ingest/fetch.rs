//! ERCOT MIS download client (`ureq`), used only by the `batsim-ercot-ingest`
//! binary. Never on a simulation path (spec scope rule: no network I/O in
//! the sim loop).
//!
//! Endpoints (verified 2026-07-27; see crate README § Verification log):
//! - Document list:
//!   `https://www.ercot.com/misapp/servlets/IceDocListJsonWS?reportTypeId=<id>`
//! - Download:
//!   `https://www.ercot.com/misdownload/servlets/mirDownload?doclookupId=<DocID>`

use std::path::{Path, PathBuf};

use crate::error::{ErcotError, Result};

/// MIS document-list endpoint.
pub const DOC_LIST_URL: &str = "https://www.ercot.com/misapp/servlets/IceDocListJsonWS";
/// MIS document-download endpoint.
pub const DOWNLOAD_URL: &str = "https://www.ercot.com/misdownload/servlets/mirDownload";

/// Maximum download size accepted for one report file (256 MiB; the yearly
/// RTM xlsx is ~22 MiB).
pub const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// One document advertised by the MIS document list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MisDocument {
    /// `DocID` used by the download endpoint.
    pub doc_id: u64,
    /// Human-readable name, e.g. "Historical RTM Load Zone and Hub Prices 2023".
    pub friendly_name: String,
}

/// Build the HTTP agent used by the ingest binary (generous global timeout
/// for multi-MiB report downloads).
#[must_use]
pub fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(300)))
        .build()
        .new_agent()
}

/// List the documents ERCOT currently advertises for a report type ID.
///
/// # Errors
/// `Fetch` on HTTP failure, non-JSON response, or an unexpected document
/// list shape.
pub fn list_documents(agent: &ureq::Agent, report_type_id: u32) -> Result<Vec<MisDocument>> {
    let url = format!("{DOC_LIST_URL}?reportTypeId={report_type_id}");
    let mut response = agent
        .get(&url)
        .call()
        .map_err(|e| ErcotError::Fetch(format!("GET {url}: {e}")))?;
    let body: serde_json::Value = response
        .body_mut()
        .read_json()
        .map_err(|e| ErcotError::Fetch(format!("GET {url}: not JSON: {e}")))?;
    let list = body
        .pointer("/ListDocsByRptTypeRes/DocumentList")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            ErcotError::Fetch(format!("GET {url}: missing ListDocsByRptTypeRes.DocumentList"))
        })?;
    let mut docs = Vec::with_capacity(list.len());
    for (i, item) in list.iter().enumerate() {
        let doc = item.get("Document").unwrap_or(item);
        let doc_id = doc
            .get("DocID")
            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
            .ok_or_else(|| ErcotError::Fetch(format!("GET {url}: entry {i} missing DocID")))?;
        let friendly_name = doc
            .get("FriendlyName")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        docs.push(MisDocument {
            doc_id,
            friendly_name,
        });
    }
    Ok(docs)
}

/// Pick the document for a calendar year: prefer a `FriendlyName` ending in
/// the year, else any name containing it; ties break to the highest `DocID`
/// (latest publication).
///
/// # Errors
/// `Fetch` when no document matches the year.
pub fn find_year_document(docs: &[MisDocument], year: i32) -> Result<MisDocument> {
    let year_str = year.to_string();
    let mut candidates: Vec<&MisDocument> = docs
        .iter()
        .filter(|d| d.friendly_name.ends_with(&year_str))
        .collect();
    if candidates.is_empty() {
        candidates = docs
            .iter()
            .filter(|d| d.friendly_name.contains(&year_str))
            .collect();
    }
    candidates
        .iter()
        .max_by_key(|d| d.doc_id)
        .map(|d| (*d).clone())
        .ok_or_else(|| {
            ErcotError::Fetch(format!(
                "no MIS document for year {year} among {} candidates",
                docs.len()
            ))
        })
}

/// Download a document into `dest_dir`; returns the local file path.
///
/// The file name derives from `FriendlyName` (sanitized); an extension is
/// appended from the payload magic when the name lacks one.
///
/// # Errors
/// - `Fetch`: HTTP failure or payload over [`MAX_DOWNLOAD_BYTES`].
/// - `Io`: cannot create `dest_dir` or write the file.
pub fn download_document(
    agent: &ureq::Agent,
    doc: &MisDocument,
    dest_dir: &Path,
) -> Result<PathBuf> {
    let url = format!("{}?doclookupId={}", DOWNLOAD_URL, doc.doc_id);
    let mut response = agent
        .get(&url)
        .call()
        .map_err(|e| ErcotError::Fetch(format!("GET {url}: {e}")))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD_BYTES)
        .read_to_vec()
        .map_err(|e| ErcotError::Fetch(format!("GET {url}: body: {e}")))?;
    let mut name = sanitize_filename(&doc.friendly_name);
    if name.is_empty() {
        name = format!("report-{}", doc.doc_id);
    }
    if Path::new(&name).extension().is_none() {
        name.push_str(match super::parse::ReportFormat::sniff(&bytes) {
            super::parse::ReportFormat::Xlsx => ".xlsx",
            super::parse::ReportFormat::Csv => ".csv",
            super::parse::ReportFormat::CsvZip => ".zip",
        });
    }
    std::fs::create_dir_all(dest_dir)?;
    let path = dest_dir.join(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Keep alphanumerics, `.`, `-`, `_`; collapse everything else to `_`.
fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_underscore = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
            out.push(c);
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    out.trim_matches('_').to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn doc(id: u64, name: &str) -> MisDocument {
        MisDocument {
            doc_id: id,
            friendly_name: name.to_string(),
        }
    }

    #[test]
    fn find_year_prefers_suffix_then_contains_then_latest_id() {
        let docs = vec![
            doc(1, "Historical RTM Load Zone and Hub Prices 2022"),
            doc(2, "Historical RTM Load Zone and Hub Prices 2023"),
            doc(3, "Historical RTM Load Zone and Hub Prices 2023 (repost)"),
            doc(4, "2023 Q4 correction"),
        ];
        // Suffix match wins over contains; highest DocID breaks the tie
        // between entries ending in "2023" (only #2 does).
        assert_eq!(find_year_document(&docs, 2023).unwrap().doc_id, 2);
        let docs2 = vec![doc(1, "RTM prices 2023 v1"), doc(5, "RTM prices 2023 v2")];
        assert_eq!(find_year_document(&docs2, 2023).unwrap().doc_id, 5);
        assert!(find_year_document(&docs, 1999).is_err());
    }

    #[test]
    fn sanitize_filename_collapses_specials() {
        assert_eq!(
            sanitize_filename("Historical RTM Load Zone and Hub Prices 2023.xlsx"),
            "Historical_RTM_Load_Zone_and_Hub_Prices_2023.xlsx"
        );
        assert_eq!(sanitize_filename("a/b\\c:d"), "a_b_c_d");
        assert_eq!(sanitize_filename("..."), "...");
        assert_eq!(sanitize_filename("  "), "");
    }
}
