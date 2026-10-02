// Package filter implements the search/filter query language:
//
//	query   := or
//	or      := and { OR and }
//	and     := unary { [AND] unary }        (juxtaposition means AND)
//	unary   := NOT unary | "(" query ")" | cmp | word
//	cmp     := field op value
//	op      := = | != | > | >= | < | <= | contains | matches | in | ~
//	value   := word | "quoted string" | "(" value { "," value } ")"
//
// A bare word matches the name: substring (case-insensitive), or a glob if it
// contains * ? or [. Parsing is independent of the UI and fully testable.
package filter

import (
	"fmt"
	"strings"
	"unicode"
)

type tokKind int

const (
	tEOF tokKind = iota
	tWord
	tString
	tOp
	tLParen
	tRParen
	tComma
)

type token struct {
	kind tokKind
	val  string
	pos  int
}

func lex(s string) ([]token, error) {
	var out []token
	i := 0
	for i < len(s) {
		c := s[i]
		switch {
		case c == ' ' || c == '\t' || c == '\n' || c == '\r':
			i++
		case c == '(':
			out = append(out, token{tLParen, "(", i})
			i++
		case c == ')':
			out = append(out, token{tRParen, ")", i})
			i++
		case c == ',':
			out = append(out, token{tComma, ",", i})
			i++
		case c == '"' || c == '\'':
			q := c
			start := i
			i++
			var b strings.Builder
			for i < len(s) && s[i] != q {
				if s[i] == '\\' && i+1 < len(s) {
					i++
				}
				b.WriteByte(s[i])
				i++
			}
			if i >= len(s) {
				return nil, fmt.Errorf("unterminated string at %d", start+1)
			}
			i++
			out = append(out, token{tString, b.String(), start})
		case strings.ContainsRune("=!<>~&|", rune(c)):
			start := i
			op := string(c)
			if i+1 < len(s) && (s[i+1] == '=' || (c == '&' && s[i+1] == '&') || (c == '|' && s[i+1] == '|')) {
				op += string(s[i+1])
			}
			i += len(op)
			switch op {
			case "&&":
				out = append(out, token{tWord, "AND", start})
			case "||":
				out = append(out, token{tWord, "OR", start})
			case "!":
				out = append(out, token{tWord, "NOT", start})
			case "&", "|":
				return nil, fmt.Errorf("unexpected %q at %d", op, start+1)
			case "==":
				out = append(out, token{tOp, "=", start})
			default:
				out = append(out, token{tOp, op, start})
			}
		default:
			start := i
			for i < len(s) && !unicode.IsSpace(rune(s[i])) && !strings.ContainsRune("()=!<>~,\"'&|", rune(s[i])) {
				i++
			}
			out = append(out, token{tWord, s[start:i], start})
		}
	}
	return append(out, token{tEOF, "", len(s)}), nil
}
