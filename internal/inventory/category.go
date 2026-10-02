package inventory

import "strings"

// Category is a coarse file classification used for colouring and stats.
type Category uint8

const (
	CatOther Category = iota
	CatImage
	CatVideo
	CatAudio
	CatArchive
	CatDiskImage
	CatDatabase
	CatCode
	CatDocument
	CatExecutable
	CatLog
	CatCache
	NumCategories
)

var categoryNames = [NumCategories]string{
	"Other", "Image", "Video", "Audio", "Archive", "Disk image", "Database",
	"Source code", "Document", "Executable", "Log", "Cache",
}

func (c Category) String() string {
	if c < NumCategories {
		return categoryNames[c]
	}
	return "Other"
}

// ParseCategory resolves a category by (case-insensitive) name or prefix.
func ParseCategory(s string) (Category, bool) {
	s = strings.ToLower(strings.TrimSpace(s))
	for i, n := range categoryNames {
		ln := strings.ToLower(n)
		if ln == s || strings.ReplaceAll(ln, " ", "") == s || strings.HasPrefix(ln, s) && len(s) >= 3 {
			return Category(i), true
		}
	}
	switch s {
	case "vm", "virtual machine", "disk":
		return CatDiskImage, true
	case "db":
		return CatDatabase, true
	case "src", "source":
		return CatCode, true
	case "exe", "binary":
		return CatExecutable, true
	}
	return 0, false
}

var extCategory = map[string]Category{}

func init() {
	add := func(c Category, exts string) {
		for _, e := range strings.Fields(exts) {
			extCategory[e] = c
		}
	}
	add(CatImage, "jpg jpeg png gif bmp tif tiff webp heic heif raw cr2 cr3 nef arw dng svg ico psd xcf avif jxl orf rw2")
	add(CatVideo, "mp4 mkv mov avi wmv flv webm m4v mpg mpeg ts m2ts vob 3gp ogv mts")
	add(CatAudio, "mp3 flac wav ogg oga opus m4a aac wma aiff aif alac mid midi")
	add(CatArchive, "zip tar gz tgz bz2 tbz xz txz zst zstd 7z rar lz lz4 lzma cab deb rpm apk jar war cpio pkg snap whl gem")
	add(CatDiskImage, "iso img qcow qcow2 vmdk vdi vhd vhdx hdd ova ovf dmg raw vmem vmsn nvram sav")
	add(CatDatabase, "db sqlite sqlite3 db3 mdb accdb frm ibd myd myi ldf mdf ndf dbf wal kdbx realm lmdb rdb aof")
	add(CatCode, "c h cc cpp cxx hpp hh go rs py pyc js mjs cjs ts tsx jsx java kt kts scala rb php pl pm sh bash zsh fish lua swift m mm cs fs vb r jl hs ml ex exs erl clj dart sql css scss sass less html htm vue svelte json yaml yml toml xml proto gradle cmake mk ipynb o a lib obj class")
	add(CatDocument, "pdf doc docx odt rtf txt md rst tex xls xlsx ods csv tsv ppt pptx odp epub mobi djvu pages numbers key org adoc")
	add(CatExecutable, "exe dll so dylib bin msi app elf ko sys efi appimage com")
	add(CatLog, "log journal")
	add(CatCache, "cache tmp temp swp bak old part crdownload")
}

// CategoryFor classifies a file by its (lower-case) extension and by
// whether it lives beneath a cache or log directory.
func CategoryFor(ext string, inCache, inLog bool) Category {
	c, ok := extCategory[ext]
	switch {
	case inCache && (!ok || c == CatOther || c == CatCode):
		return CatCache
	case inLog && (!ok || c == CatArchive || c == CatOther):
		return CatLog
	case ok:
		return c
	}
	return CatOther
}

// PathHints reports whether a directory path contains a cache or log
// component. It is evaluated once per directory, not per file.
func PathHints(path string) (inCache, inLog bool) {
	lp := strings.ToLower(path)
	for _, part := range strings.FieldsFunc(lp, func(r rune) bool { return r == '/' || r == '\\' }) {
		switch part {
		case "cache", ".cache", "caches", "__pycache__", ".npm", ".gradle", "inetcache", "temp", "tmp":
			inCache = true
		case "log", "logs", ".logs", "journal":
			inLog = true
		}
	}
	return
}

// Ext returns the lower-case extension of name without the dot ("" if none,
// or if the name is a dotfile like ".bashrc").
func Ext(name string) string {
	i := strings.LastIndexByte(name, '.')
	if i <= 0 || i == len(name)-1 || len(name)-i > 16 {
		return ""
	}
	return strings.ToLower(name[i+1:])
}
