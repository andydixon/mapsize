use std::collections::HashMap;
use std::sync::OnceLock;

/// Coarse file classification used for colouring and stats.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[repr(u8)]
pub enum Category {
    #[default]
    Other,
    Image,
    Video,
    Audio,
    Archive,
    DiskImage,
    Database,
    Code,
    Document,
    Executable,
    Log,
    Cache,
}

pub const NUM_CATEGORIES: usize = 12;

const CATEGORY_NAMES: [&str; NUM_CATEGORIES] = [
    "Other", "Image", "Video", "Audio", "Archive", "Disk image", "Database",
    "Source code", "Document", "Executable", "Log", "Cache",
];

const ALL: [Category; NUM_CATEGORIES] = [
    Category::Other, Category::Image, Category::Video, Category::Audio, Category::Archive,
    Category::DiskImage, Category::Database, Category::Code, Category::Document,
    Category::Executable, Category::Log, Category::Cache,
];

impl Category {
    pub fn from_u8(v: u8) -> Category {
        ALL.get(v as usize).copied().unwrap_or(Category::Other)
    }
    pub fn all() -> &'static [Category; NUM_CATEGORIES] {
        &ALL
    }
    pub fn name(self) -> &'static str {
        CATEGORY_NAMES[self as usize]
    }
}

impl std::fmt::Display for Category {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Resolves a category by (case-insensitive) name or prefix.
pub fn parse_category(s: &str) -> Option<Category> {
    let s = s.trim().to_lowercase();
    for (i, n) in CATEGORY_NAMES.iter().enumerate() {
        let ln = n.to_lowercase();
        if ln == s || ln.replace(' ', "") == s || ln.starts_with(&s) && s.len() >= 3 {
            return Some(ALL[i]);
        }
    }
    match s.as_str() {
        "vm" | "virtual machine" | "disk" => Some(Category::DiskImage),
        "db" => Some(Category::Database),
        "src" | "source" => Some(Category::Code),
        "exe" | "binary" => Some(Category::Executable),
        _ => None,
    }
}

fn ext_table() -> &'static HashMap<&'static str, Category> {
    static T: OnceLock<HashMap<&'static str, Category>> = OnceLock::new();
    T.get_or_init(|| {
        let mut m = HashMap::new();
        let mut add = |c: Category, exts: &'static str| {
            for e in exts.split_whitespace() {
                m.insert(e, c);
            }
        };
        add(Category::Image, "jpg jpeg png gif bmp tif tiff webp heic heif raw cr2 cr3 nef arw dng svg ico psd xcf avif jxl orf rw2");
        add(Category::Video, "mp4 mkv mov avi wmv flv webm m4v mpg mpeg ts m2ts vob 3gp ogv mts");
        add(Category::Audio, "mp3 flac wav ogg oga opus m4a aac wma aiff aif alac mid midi");
        add(Category::Archive, "zip tar gz tgz bz2 tbz xz txz zst zstd 7z rar lz lz4 lzma cab deb rpm apk jar war cpio pkg snap whl gem");
        add(Category::DiskImage, "iso img qcow qcow2 vmdk vdi vhd vhdx hdd ova ovf dmg raw vmem vmsn nvram sav");
        add(Category::Database, "db sqlite sqlite3 db3 mdb accdb frm ibd myd myi ldf mdf ndf dbf wal kdbx realm lmdb rdb aof");
        add(Category::Code, "c h cc cpp cxx hpp hh go rs py pyc js mjs cjs ts tsx jsx java kt kts scala rb php pl pm sh bash zsh fish lua swift m mm cs fs vb r jl hs ml ex exs erl clj dart sql css scss sass less html htm vue svelte json yaml yml toml xml proto gradle cmake mk ipynb o a lib obj class");
        add(Category::Document, "pdf doc docx odt rtf txt md rst tex xls xlsx ods csv tsv ppt pptx odp epub mobi djvu pages numbers key org adoc");
        add(Category::Executable, "exe dll so dylib bin msi app elf ko sys efi appimage com");
        add(Category::Log, "log journal");
        add(Category::Cache, "cache tmp temp swp bak old part crdownload");
        m
    })
}

/// Classifies a file by its (lower-case) extension and by whether it lives
/// beneath a cache or log directory.
pub fn category_for(ext: &str, in_cache: bool, in_log: bool) -> Category {
    let c = ext_table().get(ext).copied();
    match c {
        _ if in_cache && matches!(c, None | Some(Category::Other) | Some(Category::Code)) => Category::Cache,
        _ if in_log && matches!(c, None | Some(Category::Archive) | Some(Category::Other)) => Category::Log,
        Some(c) => c,
        None => Category::Other,
    }
}

/// Reports whether a directory path contains a cache or log component. It is
/// evaluated once per directory, not per file.
pub fn path_hints(path: &[u8]) -> (bool, bool) {
    let (mut in_cache, mut in_log) = (false, false);
    for part in path.split(|&b| b == b'/' || b == b'\\') {
        if part.is_empty() || part.len() > 12 {
            continue;
        }
        match part.to_ascii_lowercase().as_slice() {
            b"cache" | b".cache" | b"caches" | b"__pycache__" | b".npm" | b".gradle" | b"inetcache" | b"temp" | b"tmp" => in_cache = true,
            b"log" | b"logs" | b".logs" | b"journal" => in_log = true,
            _ => {}
        }
    }
    (in_cache, in_log)
}

/// Returns the lower-case extension of name without the dot ("" if none, or
/// if the name is a dotfile like ".bashrc").
pub fn ext(name: &[u8]) -> String {
    match name.iter().rposition(|&b| b == b'.') {
        Some(i) if i > 0 && i != name.len() - 1 && name.len() - i <= 16 => {
            String::from_utf8_lossy(&name[i + 1..]).to_lowercase()
        }
        _ => String::new(),
    }
}
