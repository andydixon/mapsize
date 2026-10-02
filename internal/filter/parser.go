package filter

import (
	"fmt"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/textutil"
)

// Field is a filterable attribute.
type Field int

const (
	FName Field = iota
	FPath
	FExt
	FType
	FCategory
	FSize
	FAllocated
	FAge
	FModified
	FOwner
	FGroup
	FFiles
	FFlag
)

var fieldNames = map[string]Field{
	"name": FName, "path": FPath, "ext": FExt, "extension": FExt, "type": FType, "kind": FType,
	"category": FCategory, "cat": FCategory, "size": FSize, "allocated": FAllocated, "alloc": FAllocated,
	"age": FAge, "modified": FModified, "mtime": FModified, "owner": FOwner, "user": FOwner, "group": FGroup,
	"files": FFiles, "flag": FFlag, "is": FFlag,
}

// Fields lists the field names for help text.
const Fields = "name path ext type category size allocated age modified owner group files flag"

// Query is a compiled filter.
type Query struct {
	src      string
	root     node
	UsesPath bool
}

func (q *Query) String() string { return q.src }

type parser struct {
	toks []token
	i    int
	q    *Query
	now  time.Time
}

// Parse compiles a query. now anchors relative ages (pass time.Now()).
func Parse(s string, now time.Time) (*Query, error) {
	toks, err := lex(s)
	if err != nil {
		return nil, err
	}
	q := &Query{src: s}
	p := &parser{toks: toks, q: q, now: now}
	if p.peek().kind == tEOF {
		return nil, fmt.Errorf("empty query")
	}
	q.root, err = p.or()
	if err != nil {
		return nil, err
	}
	if t := p.peek(); t.kind != tEOF {
		return nil, fmt.Errorf("unexpected %q at %d", t.val, t.pos+1)
	}
	return q, nil
}

func (p *parser) peek() token { return p.toks[p.i] }
func (p *parser) next() token {
	t := p.toks[p.i]
	if t.kind != tEOF {
		p.i++
	}
	return t
}

func isKw(t token, kw string) bool { return t.kind == tWord && strings.EqualFold(t.val, kw) }

func (p *parser) or() (node, error) {
	l, err := p.and()
	if err != nil {
		return nil, err
	}
	for isKw(p.peek(), "OR") {
		p.next()
		r, err := p.and()
		if err != nil {
			return nil, err
		}
		l = orNode{l, r}
	}
	return l, nil
}

func (p *parser) and() (node, error) {
	l, err := p.unary()
	if err != nil {
		return nil, err
	}
	for {
		t := p.peek()
		if isKw(t, "AND") {
			p.next()
		} else if t.kind == tEOF || t.kind == tRParen || isKw(t, "OR") {
			return l, nil
		}
		r, err := p.unary()
		if err != nil {
			return nil, err
		}
		l = andNode{l, r}
	}
}

func (p *parser) unary() (node, error) {
	t := p.peek()
	switch {
	case isKw(t, "NOT"):
		p.next()
		n, err := p.unary()
		if err != nil {
			return nil, err
		}
		return notNode{n}, nil
	case t.kind == tLParen:
		p.next()
		n, err := p.or()
		if err != nil {
			return nil, err
		}
		if p.next().kind != tRParen {
			return nil, fmt.Errorf("missing ) for ( at %d", t.pos+1)
		}
		return n, nil
	case t.kind == tWord || t.kind == tString:
		p.next()
		if f, ok := fieldNames[strings.ToLower(t.val)]; ok && t.kind == tWord && p.isOp(p.peek()) {
			return p.cmp(f, p.next())
		}
		return nameMatch(t.val), nil
	}
	return nil, fmt.Errorf("unexpected %q at %d", t.val, t.pos+1)
}

func (p *parser) isOp(t token) bool {
	return t.kind == tOp || isKw(t, "contains") || isKw(t, "matches") || isKw(t, "in")
}

func nameMatch(v string) node {
	lv := strings.ToLower(v)
	if strings.ContainsAny(v, "*?[") {
		return globNode{field: FName, pat: lv}
	}
	return containsNode{field: FName, sub: lv}
}

func (p *parser) values() ([]token, error) {
	if p.peek().kind != tLParen {
		t := p.next()
		if t.kind != tWord && t.kind != tString {
			return nil, fmt.Errorf("expected value at %d", t.pos+1)
		}
		return []token{t}, nil
	}
	p.next()
	var out []token
	for {
		t := p.next()
		if t.kind != tWord && t.kind != tString {
			return nil, fmt.Errorf("expected value at %d", t.pos+1)
		}
		out = append(out, t)
		switch n := p.next(); n.kind {
		case tComma:
		case tRParen:
			return out, nil
		default:
			return nil, fmt.Errorf("expected , or ) at %d", n.pos+1)
		}
	}
}

func (p *parser) cmp(f Field, opTok token) (node, error) {
	op := strings.ToLower(opTok.val)
	if op == "~" {
		op = "matches"
	}
	vals, err := p.values()
	if err != nil {
		return nil, err
	}
	if op != "in" && len(vals) != 1 {
		return nil, fmt.Errorf("operator %s takes one value", op)
	}
	if f == FPath {
		p.q.UsesPath = true
	}
	switch f {
	case FSize, FAllocated, FAge, FModified, FFiles:
		if op == "contains" || op == "matches" {
			return nil, fmt.Errorf("%s cannot be used with numeric field", op)
		}
		var nums []int64
		for _, v := range vals {
			n, err := p.number(f, v.val)
			if err != nil {
				return nil, err
			}
			nums = append(nums, n)
		}
		if op == "in" {
			op = "="
		}
		return numNode{field: f, op: op, vals: nums}, nil
	}
	// String-like fields.
	switch op {
	case "contains":
		return containsNode{field: f, sub: strings.ToLower(vals[0].val)}, nil
	case "matches":
		re, err := regexp.Compile("(?i)" + vals[0].val)
		if err != nil {
			return nil, fmt.Errorf("bad regular expression: %v", err)
		}
		return reNode{field: f, re: re}, nil
	case "=", "!=", "in":
		var n node
		var set []string
		for _, v := range vals {
			s := strings.ToLower(v.val)
			if f == FExt {
				s = strings.TrimPrefix(s, ".")
			}
			if f == FCategory {
				c, ok := inventory.ParseCategory(s)
				if !ok {
					return nil, fmt.Errorf("unknown category %q", v.val)
				}
				s = strings.ToLower(c.String())
			}
			if f == FType {
				s, err = normType(s)
				if err != nil {
					return nil, err
				}
			}
			if f == FFlag {
				if _, ok := flagNames[s]; !ok {
					return nil, fmt.Errorf("unknown flag %q (sparse, hardlink, error, mount, incomplete, followed, broken)", v.val)
				}
			}
			set = append(set, s)
		}
		if len(set) == 1 && strings.ContainsAny(set[0], "*?[") {
			if _, err := filepath.Match(set[0], ""); err != nil {
				return nil, fmt.Errorf("bad pattern %q", vals[0].val)
			}
			n = globNode{field: f, pat: set[0]}
		} else {
			n = eqNode{field: f, set: set}
		}
		if op == "!=" {
			n = notNode{n}
		}
		return n, nil
	}
	return nil, fmt.Errorf("operator %s not valid for %s", op, opTok.val)
}

func normType(s string) (string, error) {
	switch s {
	case "f", "file":
		return "file", nil
	case "d", "dir", "directory":
		return "directory", nil
	case "l", "link", "symlink":
		return "symlink", nil
	case "other", "special":
		return "special", nil
	}
	return "", fmt.Errorf("unknown type %q (file, dir, symlink, other)", s)
}

var flagNames = map[string]inventory.Flags{
	"sparse": inventory.FlagSparse, "hardlink": inventory.FlagHardlinked, "hardlinked": inventory.FlagHardlinked,
	"error": inventory.FlagError, "mount": inventory.FlagMountPoint, "incomplete": inventory.FlagIncomplete,
	"followed": inventory.FlagFollowed, "broken": inventory.FlagBrokenLink,
}

// number parses sizes ("1.5GiB", "500MB", "10k"), ages ("365d", "2w") and
// dates ("2024-01-31"). Ages and dates are converted to modification times
// (unix nanoseconds) relative to p.now.
func (p *parser) number(f Field, s string) (int64, error) {
	switch f {
	case FSize, FAllocated:
		return ParseSize(s)
	case FFiles:
		return strconv.ParseInt(s, 10, 64)
	case FAge:
		d, err := ParseAge(s)
		return int64(d), err
	case FModified:
		for _, layout := range []string{"2006-01-02", "2006-01-02T15:04", "2006-01-02 15:04", time.RFC3339} {
			if t, err := time.ParseInLocation(layout, s, time.Local); err == nil {
				return t.UnixNano(), nil
			}
		}
		if d, err := ParseAge(s); err == nil {
			return p.now.Add(-d).UnixNano(), nil
		}
		return 0, fmt.Errorf("bad date %q (use YYYY-MM-DD or an age like 30d)", s)
	}
	return 0, fmt.Errorf("not numeric")
}

// ParseSize parses a size with optional unit. kB/MB/GB/TB are SI (powers of
// 1000); KiB/MiB/GiB/TiB are IEC (1024). Bare K/M/G/T follow the display
// setting (textutil.SI).
func ParseSize(s string) (int64, error) {
	i := 0
	for i < len(s) && (s[i] >= '0' && s[i] <= '9' || s[i] == '.') {
		i++
	}
	if i == 0 {
		return 0, fmt.Errorf("bad size %q", s)
	}
	v, err := strconv.ParseFloat(s[:i], 64)
	if err != nil {
		return 0, fmt.Errorf("bad size %q", s)
	}
	unit := strings.ToLower(strings.TrimSpace(s[i:]))
	bare := 1024.0
	if textutil.SI {
		bare = 1000
	}
	mult := map[string]float64{"": 1, "b": 1,
		"k": bare, "m": bare * bare, "g": bare * bare * bare, "t": bare * bare * bare * bare, "p": bare * bare * bare * bare * bare,
		"kb": 1e3, "mb": 1e6, "gb": 1e9, "tb": 1e12, "pb": 1e15,
		"kib": 1 << 10, "mib": 1 << 20, "gib": 1 << 30, "tib": 1 << 40, "pib": 1 << 50}
	m, ok := mult[unit]
	if !ok {
		return 0, fmt.Errorf("unknown size unit %q", s[i:])
	}
	r := v * m
	if r > 9e18 {
		return 0, fmt.Errorf("size %q too large", s)
	}
	return int64(r), nil
}

// ParseAge parses durations like 90s, 30min, 12h, 7d, 2w, 6mo, 1y.
func ParseAge(s string) (time.Duration, error) {
	i := 0
	for i < len(s) && (s[i] >= '0' && s[i] <= '9' || s[i] == '.') {
		i++
	}
	v, err := strconv.ParseFloat(s[:i], 64)
	if err != nil {
		return 0, fmt.Errorf("bad age %q", s)
	}
	day := 24 * time.Hour
	units := map[string]time.Duration{"s": time.Second, "sec": time.Second, "min": time.Minute,
		"h": time.Hour, "d": day, "": day, "w": 7 * day, "m": 30 * day, "mo": 30 * day, "y": 365 * day}
	u, ok := units[strings.ToLower(s[i:])]
	if !ok {
		return 0, fmt.Errorf("unknown age unit %q (s, min, h, d, w, mo, y)", s[i:])
	}
	return time.Duration(v * float64(u)), nil
}
