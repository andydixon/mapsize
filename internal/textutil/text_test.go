package textutil

import (
	"strings"
	"testing"
)

func TestSanitize(t *testing.T) {
	cases := map[string]string{
		"plain.txt":               "plain.txt",
		"evil\x1b[2Jname":         `evil\x1b[2Jname`,
		"bell\a":                  `bell\x07`,
		"c1\u009bx":               `c1\x9bx`,
		"bad\xffutf8":             "bad�utf8",
		"rtl\u202egnp.exe":        `rtl\u202egnp.exe`,
		"日本語":                     "日本語",
		"tab\tnew\nline\rdel\x7f": `tab\x09new\x0aline\x0ddel\x7f`,
	}
	for in, want := range cases {
		if got := Sanitize(in); got != want {
			t.Errorf("Sanitize(%q) = %q, want %q", in, got, want)
		}
		if strings.ContainsAny(Sanitize(in), "\x1b\a\u009b") {
			t.Errorf("control byte survived in %q", in)
		}
	}
}

func TestTruncate(t *testing.T) {
	if got := Truncate("hello world", 5); got != "hell…" {
		t.Errorf("got %q", got)
	}
	if got := Truncate("日本語テキスト", 5); Width(got) > 5 {
		t.Errorf("wide truncation overflow: %q width %d", got, Width(got))
	}
	if got := Truncate("abc", 3); got != "abc" {
		t.Errorf("got %q", got)
	}
	if got := Truncate("abc", 0); got != "" {
		t.Errorf("got %q", got)
	}
	if got := TruncateLeft("/a/very/long/path", 8); Width(got) != 8 || !strings.HasSuffix(got, "path") {
		t.Errorf("TruncateLeft got %q", got)
	}
	if got := TruncateLeft("ab日本語", 4); Width(got) != 4 {
		t.Errorf("TruncateLeft wide got %q (%d)", got, Width(got))
	}
}

func TestSize(t *testing.T) {
	cases := map[int64]string{0: "0 B", 1023: "1023 B", 1024: "1.00 KiB", 1536: "1.50 KiB",
		181 << 30: "181 GiB", 1 << 62: "4.00 EiB"}
	for in, want := range cases {
		if got := Size(in); got != want {
			t.Errorf("Size(%d)=%q want %q", in, got, want)
		}
	}
	if got := Count(2184392); got != "2,184,392" {
		t.Errorf("Count got %q", got)
	}
	if got := SignedSize(-2048); got != "-2.00 KiB" {
		t.Errorf("SignedSize got %q", got)
	}
}

func TestClustersAgreeWithWidth(t *testing.T) {
	for _, s := range []string{"abc", "日本", "é", "👨‍👩‍👧", "🎉x", "​"} {
		n := 0
		Clusters(s, func(_ string, w int) bool { n += w; return true })
		if n != Width(s) {
			t.Errorf("%q: clusters %d width %d", s, n, Width(s))
		}
	}
}
