//! Fixed-length property and edge record store.
//!
//! Each record occupies 16 bytes, addressed directly by `PropId`
//! (`offset = HEADER_LEN + id × 16`), giving O(1) access.
//!
//! On-disk segment layout (little-endian throughout):
//! ```text
//! [Header  16 B]   magic(4) + version(2) + flags(2) + record_size(2) + id_bytes(1) + pad(5)
//! [Record  16 B]*  one per property/edge, in PropId order
//! [Footer  20 B]   count(8) + flags(4) + crc32(4) + magic(4)
//! ```
//!
//! Record layout:
//! ```text
//! byte  0      PropType (u8)    value-type discriminant
//! byte  1      extra    (u8)    InlineStr: byte length; others: 0
//! bytes 2-15   data     [14]   value bytes (see per-type encoding below)
//! ```
//!
//! Per-type value encoding (all integers little-endian):
//! ```text
//! Edge      data[0..4] = end_node_id (u32)
//! None      data = zeros
//! Bool      data[0] = 0 (false) or 1 (true)
//! I64       data[0..8] = i64
//! I128Ref   data[0..8] = StrId → 16-byte big integer in String Store
//! F64       data[0..8] = f64 bits
//! F128Ref   data[0..8] = StrId → 16-byte float in String Store
//! Date      data[0..8] = days since 1970-01-01 as i64
//! Time      data[0..8] = nanoseconds since midnight as u64
//! DateTime  data[0..8] = seconds since Unix epoch as i64
//! Duration  data[0..8] = seconds as i64
//! Uuid      data[0..8] = StrId → 16-byte UUID in String Store
//! Hash      data[0..8] = StrId → hash bytes in String Store
//! Geo       data[0..8] = StrId → lat(f64)+lon(f64) packed in String Store
//! InlineStr extra = byte length (0-14), data[0..extra] = UTF-8 bytes
//! StringRef data[0..8] = StrId → string in String Store
//! UrlRef    data[0..8] = StrId → URL in String Store
//! ```
//!
//! Values requiring 16 bytes (i128, UUID, GEO, hashes) are stored in the
//! String Store and referenced by `StrId` (REQ-37). The linked-list `next`
//! pointer for graph traversal lives in the corresponding `NodeRecord.first_out`
//! field in the Node Store, keeping the full 16 bytes available for value data.

use crate::node_store::NodeId;
use crate::string_store::StrId;
use std::io;
use std::path::Path;

const HEADER_MAGIC: u32 = u32::from_le_bytes(*b"MGPS"); // mGraphDB Property Store
const FOOTER_MAGIC: u32 = u32::from_le_bytes(*b"MGPF"); // mGraphDB Property Footer
const FORMAT_VERSION: u16 = 1;
const RECORD_SIZE: usize = 16;
const ID_BYTES: u8 = 4;
const HEADER_LEN: usize = 16;
const FOOTER_LEN: usize = 20;

/// Maximum number of UTF-8 bytes that can be stored inline in a record.
pub const INLINE_STR_MAX: usize = 14;

// ── PropType ─────────────────────────────────────────────────────────────────

/// Discriminant stored in record byte 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PropType {
    Edge      = 0,
    None      = 1,
    Bool      = 2,
    I64       = 3,
    I128Ref   = 4,  // 16-byte integer via StrId
    F64       = 5,
    F128Ref   = 6,  // 16-byte float via StrId
    Date      = 7,  // days since 1970-01-01
    Time      = 8,  // nanos since midnight
    DateTime  = 9,  // seconds since Unix epoch
    Duration  = 10, // seconds
    Uuid      = 11, // 16-byte UUID via StrId
    Hash      = 12, // hash bytes via StrId
    Geo       = 13, // lat+lon (2×f64) via StrId
    InlineStr = 14, // up to 14 UTF-8 bytes stored directly
    StringRef = 15, // long string via StrId
    UrlRef    = 16, // URL via StrId
}

impl PropType {
    fn from_u8(v: u8) -> io::Result<Self> {
        match v {
            0  => Ok(Self::Edge),
            1  => Ok(Self::None),
            2  => Ok(Self::Bool),
            3  => Ok(Self::I64),
            4  => Ok(Self::I128Ref),
            5  => Ok(Self::F64),
            6  => Ok(Self::F128Ref),
            7  => Ok(Self::Date),
            8  => Ok(Self::Time),
            9  => Ok(Self::DateTime),
            10 => Ok(Self::Duration),
            11 => Ok(Self::Uuid),
            12 => Ok(Self::Hash),
            13 => Ok(Self::Geo),
            14 => Ok(Self::InlineStr),
            15 => Ok(Self::StringRef),
            16 => Ok(Self::UrlRef),
            _  => Err(bad(&format!("unknown PropType discriminant {v}"))),
        }
    }
}

// ── PropValue ────────────────────────────────────────────────────────────────

/// Decoded property or edge value.
#[derive(Clone, Debug, PartialEq)]
pub enum PropValue {
    /// An edge pointing to `end_node` (start node is implicit — the owning node).
    Edge      { end_node: NodeId },
    /// Null / absent value.
    None,
    Bool      (bool),
    /// 64-bit signed integer.
    I64       (i64),
    /// 128-bit integer stored in the String Store (too large for inline).
    I128Ref   (StrId),
    /// 64-bit float.
    F64       (f64),
    /// 128-bit float stored in the String Store.
    F128Ref   (StrId),
    /// Days since 1970-01-01.
    Date      (i64),
    /// Nanoseconds since midnight.
    Time      (u64),
    /// Seconds since Unix epoch (1970-01-01T00:00:00Z).
    DateTime  (i64),
    /// Duration in seconds.
    Duration  (i64),
    /// 16-byte UUID stored in the String Store.
    Uuid      (StrId),
    /// Hash bytes stored in the String Store.
    Hash      (StrId),
    /// Lat + lon packed as two f64 values in the String Store.
    Geo       (StrId),
    /// Up to 14 UTF-8 bytes stored directly in the record.
    InlineStr (String),
    /// String too long for inline; stored in the String Store.
    StringRef (StrId),
    /// URL stored in the String Store.
    UrlRef    (StrId),
}

impl PropValue {
    fn prop_type(&self) -> PropType {
        match self {
            Self::Edge      { .. } => PropType::Edge,
            Self::None             => PropType::None,
            Self::Bool      (_)    => PropType::Bool,
            Self::I64       (_)    => PropType::I64,
            Self::I128Ref   (_)    => PropType::I128Ref,
            Self::F64       (_)    => PropType::F64,
            Self::F128Ref   (_)    => PropType::F128Ref,
            Self::Date      (_)    => PropType::Date,
            Self::Time      (_)    => PropType::Time,
            Self::DateTime  (_)    => PropType::DateTime,
            Self::Duration  (_)    => PropType::Duration,
            Self::Uuid      (_)    => PropType::Uuid,
            Self::Hash      (_)    => PropType::Hash,
            Self::Geo       (_)    => PropType::Geo,
            Self::InlineStr (_)    => PropType::InlineStr,
            Self::StringRef (_)    => PropType::StringRef,
            Self::UrlRef    (_)    => PropType::UrlRef,
        }
    }

    /// Encode this value into a 16-byte record. Returns an error if an
    /// `InlineStr` exceeds `INLINE_STR_MAX` bytes.
    pub fn encode(&self) -> io::Result<[u8; RECORD_SIZE]> {
        let mut rec = [0u8; RECORD_SIZE];
        rec[0] = self.prop_type() as u8;

        match self {
            Self::Edge { end_node } => {
                rec[2..6].copy_from_slice(&end_node.to_le_bytes());
            }
            Self::None => {}
            Self::Bool(v) => {
                rec[2] = *v as u8;
            }
            Self::I64(v) => {
                rec[2..10].copy_from_slice(&v.to_le_bytes());
            }
            Self::I128Ref(id) | Self::F128Ref(id) | Self::Uuid(id)
            | Self::Hash(id) | Self::Geo(id) | Self::StringRef(id)
            | Self::UrlRef(id) => {
                rec[2..10].copy_from_slice(&id.to_le_bytes());
            }
            Self::F64(v) => {
                rec[2..10].copy_from_slice(&v.to_bits().to_le_bytes());
            }
            Self::Date(v) | Self::DateTime(v) | Self::Duration(v) => {
                rec[2..10].copy_from_slice(&v.to_le_bytes());
            }
            Self::Time(v) => {
                rec[2..10].copy_from_slice(&v.to_le_bytes());
            }
            Self::InlineStr(s) => {
                let bytes = s.as_bytes();
                if bytes.len() > INLINE_STR_MAX {
                    return Err(bad(&format!(
                        "inline string too long: {} bytes (max {INLINE_STR_MAX})",
                        bytes.len()
                    )));
                }
                rec[1] = bytes.len() as u8;
                rec[2..2 + bytes.len()].copy_from_slice(bytes);
            }
        }
        Ok(rec)
    }

    /// Decode a 16-byte record into a `PropValue`.
    pub fn decode(rec: &[u8; RECORD_SIZE]) -> io::Result<Self> {
        let ptype = PropType::from_u8(rec[0])?;
        let extra = rec[1];
        let data  = &rec[2..]; // 14 bytes

        Ok(match ptype {
            PropType::Edge => {
                let end_node = u32::from_le_bytes(data[0..4].try_into().unwrap());
                Self::Edge { end_node }
            }
            PropType::None      => Self::None,
            PropType::Bool      => Self::Bool(data[0] != 0),
            PropType::I64       => Self::I64(i64::from_le_bytes(data[0..8].try_into().unwrap())),
            PropType::I128Ref   => Self::I128Ref(u64::from_le_bytes(data[0..8].try_into().unwrap())),
            PropType::F64       => Self::F64(f64::from_bits(u64::from_le_bytes(data[0..8].try_into().unwrap()))),
            PropType::F128Ref   => Self::F128Ref(u64::from_le_bytes(data[0..8].try_into().unwrap())),
            PropType::Date      => Self::Date(i64::from_le_bytes(data[0..8].try_into().unwrap())),
            PropType::Time      => Self::Time(u64::from_le_bytes(data[0..8].try_into().unwrap())),
            PropType::DateTime  => Self::DateTime(i64::from_le_bytes(data[0..8].try_into().unwrap())),
            PropType::Duration  => Self::Duration(i64::from_le_bytes(data[0..8].try_into().unwrap())),
            PropType::Uuid      => Self::Uuid(u64::from_le_bytes(data[0..8].try_into().unwrap())),
            PropType::Hash      => Self::Hash(u64::from_le_bytes(data[0..8].try_into().unwrap())),
            PropType::Geo       => Self::Geo(u64::from_le_bytes(data[0..8].try_into().unwrap())),
            PropType::StringRef => Self::StringRef(u64::from_le_bytes(data[0..8].try_into().unwrap())),
            PropType::UrlRef    => Self::UrlRef(u64::from_le_bytes(data[0..8].try_into().unwrap())),
            PropType::InlineStr => {
                let len = extra as usize;
                if len > INLINE_STR_MAX {
                    return Err(bad(&format!("inline string length {len} exceeds max {INLINE_STR_MAX}")));
                }
                let s = std::str::from_utf8(&data[0..len])
                    .map_err(|_| bad("inline string is not valid UTF-8"))?;
                Self::InlineStr(s.to_owned())
            }
        })
    }
}

// ── PropId / PropStore ────────────────────────────────────────────────────────

/// Compact property/edge record identifier (u32).
pub type PropId = u32;

/// Sentinel meaning "no reference".
pub const NO_PROP: PropId = PropId::MAX;

/// In-memory, append-friendly property/edge store backed by a single segment file.
pub struct PropStore {
    records: Vec<[u8; RECORD_SIZE]>,
}

impl Default for PropStore {
    fn default() -> Self { Self::new() }
}

impl PropStore {
    pub fn new() -> Self {
        PropStore { records: Vec::new() }
    }

    /// Total number of records.
    pub fn len(&self) -> usize { self.records.len() }

    pub fn is_empty(&self) -> bool { self.records.is_empty() }

    /// Encode and store `value`, returning its new `PropId`.
    pub fn create(&mut self, value: &PropValue) -> io::Result<PropId> {
        let id = self.records.len() as PropId;
        self.records.push(value.encode()?);
        Ok(id)
    }

    /// Decode and return the value at `id`, or `None` if out of range.
    pub fn get(&self, id: PropId) -> io::Result<Option<PropValue>> {
        match self.records.get(id as usize) {
            None => Ok(None),
            Some(rec) => PropValue::decode(rec).map(Some),
        }
    }

    /// Overwrite the record at `id`. Returns `false` if `id` is out of range.
    pub fn update(&mut self, id: PropId, value: &PropValue) -> io::Result<bool> {
        match self.records.get_mut(id as usize) {
            None => Ok(false),
            Some(slot) => { *slot = value.encode()?; Ok(true) }
        }
    }

    /// Serialize to a segment file via temp + rename.
    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let mut buf =
            Vec::with_capacity(HEADER_LEN + self.records.len() * RECORD_SIZE + FOOTER_LEN);

        // Header
        buf.extend_from_slice(&HEADER_MAGIC.to_le_bytes());
        buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&(RECORD_SIZE as u16).to_le_bytes());
        buf.push(ID_BYTES);
        buf.extend_from_slice(&[0u8; 5]);

        // Records
        let records_start = buf.len();
        for rec in &self.records {
            buf.extend_from_slice(rec);
        }
        let crc = crc32fast::hash(&buf[records_start..]);

        // Footer
        buf.extend_from_slice(&(self.records.len() as u64).to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&crc.to_le_bytes());
        buf.extend_from_slice(&FOOTER_MAGIC.to_le_bytes());

        let path = path.as_ref();
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(&tmp, path)
    }

    /// Load from a segment file, verifying all integrity checks.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let data = std::fs::read(path)?;
        if data.len() < HEADER_LEN + FOOTER_LEN {
            return Err(bad("file too small"));
        }
        if read_u32(&data, 0) != HEADER_MAGIC {
            return Err(bad("bad header magic"));
        }
        let version = read_u16(&data, 4);
        if version != FORMAT_VERSION {
            return Err(bad("unsupported format version"));
        }
        let record_size = read_u16(&data, 8) as usize;
        if record_size != RECORD_SIZE {
            return Err(bad("unexpected record size"));
        }

        let f = data.len() - FOOTER_LEN;
        if read_u32(&data, f + 16) != FOOTER_MAGIC {
            return Err(bad("bad footer magic"));
        }
        let count      = read_u64(&data, f) as usize;
        let stored_crc = read_u32(&data, f + 12);

        let records_end = HEADER_LEN + count * RECORD_SIZE;
        if records_end > f {
            return Err(bad("record region overruns footer"));
        }
        let record_bytes = &data[HEADER_LEN..records_end];
        if crc32fast::hash(record_bytes) != stored_crc {
            return Err(bad("records crc mismatch"));
        }

        let mut records = Vec::with_capacity(count);
        for i in 0..count {
            let o = i * RECORD_SIZE;
            let mut rec = [0u8; RECORD_SIZE];
            rec.copy_from_slice(&record_bytes[o..o + RECORD_SIZE]);
            records.push(rec);
        }
        Ok(PropStore { records })
    }
}

// ── Low-level helpers ─────────────────────────────────────────────────────────

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("prop store: {msg}"))
}

fn read_u16(buf: &[u8], pos: usize) -> u16 {
    u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap())
}

fn read_u32(buf: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap())
}

fn read_u64(buf: &[u8], pos: usize) -> u64 {
    u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(v: PropValue) -> PropValue {
        PropValue::decode(&v.encode().unwrap()).unwrap()
    }

    #[test]
    fn encode_decode_edge() {
        let v = PropValue::Edge { end_node: 42 };
        assert_eq!(rt(v.clone()), v);
    }

    #[test]
    fn encode_decode_none() {
        assert_eq!(rt(PropValue::None), PropValue::None);
    }

    #[test]
    fn encode_decode_bool() {
        assert_eq!(rt(PropValue::Bool(true)),  PropValue::Bool(true));
        assert_eq!(rt(PropValue::Bool(false)), PropValue::Bool(false));
    }

    #[test]
    fn encode_decode_i64() {
        for v in [0i64, 1, -1, i64::MIN, i64::MAX, 12345678] {
            assert_eq!(rt(PropValue::I64(v)), PropValue::I64(v));
        }
    }

    #[test]
    fn encode_decode_f64() {
        for v in [0.0f64, 1.0, -1.0, f64::MAX, f64::MIN_POSITIVE, std::f64::consts::PI] {
            assert_eq!(rt(PropValue::F64(v)), PropValue::F64(v));
        }
    }

    #[test]
    fn encode_decode_temporal() {
        assert_eq!(rt(PropValue::Date(19_000)),         PropValue::Date(19_000));
        assert_eq!(rt(PropValue::Time(86_399_999_999_999)), PropValue::Time(86_399_999_999_999));
        assert_eq!(rt(PropValue::DateTime(1_700_000_000)), PropValue::DateTime(1_700_000_000));
        assert_eq!(rt(PropValue::Duration(-3600)),      PropValue::Duration(-3600));
    }

    #[test]
    fn encode_decode_ref_types() {
        let id: StrId = 0xDEAD_BEEF_CAFE_1234;
        assert_eq!(rt(PropValue::I128Ref(id)),   PropValue::I128Ref(id));
        assert_eq!(rt(PropValue::F128Ref(id)),   PropValue::F128Ref(id));
        assert_eq!(rt(PropValue::Uuid(id)),      PropValue::Uuid(id));
        assert_eq!(rt(PropValue::Hash(id)),      PropValue::Hash(id));
        assert_eq!(rt(PropValue::Geo(id)),       PropValue::Geo(id));
        assert_eq!(rt(PropValue::StringRef(id)), PropValue::StringRef(id));
        assert_eq!(rt(PropValue::UrlRef(id)),    PropValue::UrlRef(id));
    }

    #[test]
    fn encode_decode_inline_str() {
        for s in ["", "hi", "hello", "héllo", "日本語", "exactly14byts!"] {
            if s.len() <= INLINE_STR_MAX {
                let v = PropValue::InlineStr(s.to_owned());
                assert_eq!(rt(v), PropValue::InlineStr(s.to_owned()));
            }
        }
    }

    #[test]
    fn inline_str_too_long_returns_error() {
        let long = "a".repeat(INLINE_STR_MAX + 1);
        assert!(PropValue::InlineStr(long).encode().is_err());
    }

    #[test]
    fn create_and_get() {
        let mut store = PropStore::new();
        let id1 = store.create(&PropValue::Bool(true)).unwrap();
        let id2 = store.create(&PropValue::I64(-99)).unwrap();
        assert_eq!(id1, 0);
        assert_eq!(id2, 1);
        assert_eq!(store.get(id1).unwrap(), Some(PropValue::Bool(true)));
        assert_eq!(store.get(id2).unwrap(), Some(PropValue::I64(-99)));
        assert_eq!(store.get(99).unwrap(), None);
    }

    #[test]
    fn update_record() {
        let mut store = PropStore::new();
        let id = store.create(&PropValue::Bool(false)).unwrap();
        assert!(store.update(id, &PropValue::Bool(true)).unwrap());
        assert_eq!(store.get(id).unwrap(), Some(PropValue::Bool(true)));
        assert!(!store.update(99, &PropValue::None).unwrap());
    }

    #[test]
    fn roundtrip_save_open() {
        let dir  = std::env::temp_dir();
        let path = dir.join(format!("mgraphdb_ps_{}.seg", std::process::id()));

        let values = [
            PropValue::Edge { end_node: 7 },
            PropValue::Bool(true),
            PropValue::I64(-42),
            PropValue::F64(std::f64::consts::E),
            PropValue::Date(18_628),
            PropValue::InlineStr("hello".to_owned()),
            PropValue::StringRef(0xABCD),
            PropValue::Uuid(0x1234_5678_9ABC_DEF0),
        ];

        let mut src = PropStore::new();
        let ids: Vec<_> = values.iter().map(|v| src.create(v).unwrap()).collect();
        src.save(&path).unwrap();

        let loaded = PropStore::open(&path).unwrap();
        assert_eq!(loaded.len(), values.len());
        for (i, v) in values.iter().enumerate() {
            assert_eq!(loaded.get(ids[i]).unwrap().as_ref(), Some(v));
        }

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn open_rejects_corrupt_header_magic() {
        let dir  = std::env::temp_dir();
        let path = dir.join(format!("mgraphdb_ps_bad_{}.seg", std::process::id()));
        let mut s = PropStore::new();
        s.create(&PropValue::None).unwrap();
        s.save(&path).unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();
        assert!(PropStore::open(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn open_rejects_corrupt_record_crc() {
        let dir  = std::env::temp_dir();
        let path = dir.join(format!("mgraphdb_ps_crc_{}.seg", std::process::id()));
        let mut s = PropStore::new();
        s.create(&PropValue::I64(42)).unwrap();
        s.save(&path).unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        bytes[HEADER_LEN] ^= 0x01;
        std::fs::write(&path, &bytes).unwrap();
        assert!(PropStore::open(&path).is_err());
        std::fs::remove_file(&path).ok();
    }
}
