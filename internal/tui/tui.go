// Package tui provides minimal raw-mode terminal widgets: an arrow-key
// selector and a masked secret input. No full-screen takeover; the widget
// redraws in place and leaves a one-line summary behind.
package tui

import (
	"errors"
	"fmt"
	"io"
	"os"
	"strings"
	"unicode/utf8"

	"golang.org/x/term"
)

var ErrCanceled = errors.New("canceled")

type Item struct {
	Label    string
	Detail   string
	Disabled bool
}

type Terminal struct {
	In    *os.File
	Out   io.Writer
	Color bool

	pending []byte
}

func New() *Terminal {
	return &Terminal{In: os.Stdin, Out: os.Stdout,
		Color: os.Getenv("NO_COLOR") == "" && os.Getenv("TERM") != "dumb"}
}

// Available reports whether interactive widgets can run.
func Available() bool {
	return term.IsTerminal(int(os.Stdin.Fd())) && term.IsTerminal(int(os.Stdout.Fd()))
}

func (t *Terminal) style(code, s string) string {
	if !t.Color || s == "" {
		return s
	}
	return "\x1b[" + code + "m" + s + "\x1b[0m"
}

func (t *Terminal) Dim(s string) string    { return t.style("2", s) }
func (t *Terminal) Bold(s string) string   { return t.style("1", s) }
func (t *Terminal) Green(s string) string  { return t.style("32", s) }
func (t *Terminal) Red(s string) string    { return t.style("31", s) }
func (t *Terminal) Yellow(s string) string { return t.style("33", s) }
func (t *Terminal) Cyan(s string) string   { return t.style("36", s) }

func (t *Terminal) Println(a ...any) { fmt.Fprintln(t.Out, a...) }

// raw switches the input to raw mode and returns a restore func.
func (t *Terminal) raw() (func(), error) {
	fd := int(t.In.Fd())
	st, err := term.MakeRaw(fd)
	if err != nil {
		return nil, err
	}
	fmt.Fprint(t.Out, "\x1b[?25l") // hide cursor
	return func() {
		fmt.Fprint(t.Out, "\x1b[?25h")
		_ = term.Restore(fd, st)
	}, nil
}

type key int

const (
	keyOther key = iota
	keyUp
	keyDown
	keyEnter
	keyCancel
	keyBackspace
	keyChar
)

// readKey decodes the next keypress from a byte stream; several keys (fast
// typing, pastes) may arrive in one read. Printable runs are returned
// together so pastes stay fast.
func (t *Terminal) readKey(buf []byte) (key, []byte, error) {
	if len(t.pending) == 0 {
		n, err := t.In.Read(buf)
		if err != nil {
			return keyOther, nil, err
		}
		t.pending = append(t.pending[:0], buf[:n]...)
	}
	b := t.pending
	consume := func(n int) { t.pending = t.pending[n:] }
	switch c := b[0]; {
	case c == 0x1b:
		if len(b) == 1 {
			consume(1)
			return keyCancel, nil, nil
		}
		if b[1] != '[' && b[1] != 'O' {
			consume(1)
			return keyCancel, nil, nil
		}
		// CSI: ESC [ params final-byte
		i := 2
		for i < len(b) && (b[i] < 0x40 || b[i] > 0x7e) {
			i++
		}
		if i >= len(b) {
			consume(len(b))
			return keyOther, nil, nil
		}
		consume(i + 1)
		switch b[i] {
		case 'A':
			return keyUp, nil, nil
		case 'B':
			return keyDown, nil, nil
		}
		return keyOther, nil, nil
	case c == 0x03:
		consume(1)
		return keyCancel, nil, nil
	case c == '\r' || c == '\n':
		consume(1)
		return keyEnter, nil, nil
	case c == 0x7f || c == 0x08:
		consume(1)
		return keyBackspace, nil, nil
	case c < 0x20:
		consume(1)
		return keyOther, nil, nil
	}
	i := 0
	for i < len(b) && b[i] >= 0x20 && b[i] != 0x7f && b[i] != 0x1b {
		i++
	}
	chunk := append([]byte(nil), b[:i]...)
	consume(i)
	return keyChar, chunk, nil
}

// Select shows items and returns the chosen index. ↑/↓ or j/k move, Enter
// selects, q/Esc/Ctrl-C cancel.
func (t *Terminal) Select(title, hint string, items []Item, start int) (int, error) {
	if len(items) == 0 {
		return 0, ErrCanceled
	}
	restore, err := t.raw()
	if err != nil {
		return 0, err
	}
	defer restore()
	cur := max(0, min(start, len(items)-1))
	width := 0
	for _, it := range items {
		width = max(width, utf8.RuneCountInString(it.Label))
	}
	lines := 0
	draw := func() {
		if lines > 0 {
			fmt.Fprintf(t.Out, "\x1b[%dA\r\x1b[J", lines)
		}
		var b strings.Builder
		b.WriteString(t.Bold(title) + "\r\n")
		for i, it := range items {
			pad := strings.Repeat(" ", width-utf8.RuneCountInString(it.Label))
			label, detail := it.Label+pad, it.Detail
			if it.Disabled {
				label, detail = t.Dim(label), t.Dim(detail)
			}
			if i == cur {
				b.WriteString(t.Cyan("❯ ") + t.Bold(label) + "  " + detail + "\r\n")
			} else {
				b.WriteString("  " + label + "  " + detail + "\r\n")
			}
		}
		b.WriteString(t.Dim(hint))
		fmt.Fprint(t.Out, b.String())
		lines = len(items) + 1
	}
	draw()
	buf := make([]byte, 64)
	for {
		k, chars, err := t.readKey(buf)
		if err != nil {
			return 0, err
		}
		switch k {
		case keyUp:
			cur = (cur - 1 + len(items)) % len(items)
		case keyDown:
			cur = (cur + 1) % len(items)
		case keyEnter:
			fmt.Fprintf(t.Out, "\x1b[%dA\r\x1b[J", lines)
			fmt.Fprintf(t.Out, "%s %s\r\n", t.Dim(title+" ›"), items[cur].Label)
			return cur, nil
		case keyCancel:
			fmt.Fprintf(t.Out, "\x1b[%dA\r\x1b[J", lines)
			return 0, ErrCanceled
		case keyChar:
			for _, c := range chars {
				switch c {
				case 'k':
					cur = (cur - 1 + len(items)) % len(items)
				case 'j':
					cur = (cur + 1) % len(items)
				case 'q':
					fmt.Fprintf(t.Out, "\x1b[%dA\r\x1b[J", lines)
					return 0, ErrCanceled
				}
			}
		}
		draw()
	}
}

// Secret reads a masked value (pasting works). Enter submits; Esc/Ctrl-C cancel.
func (t *Terminal) Secret(prompt string) (string, error) {
	restore, err := t.raw()
	if err != nil {
		return "", err
	}
	defer restore()
	var val []byte
	draw := func() {
		n := utf8.RuneCount(val)
		mask := strings.Repeat("•", min(n, 24))
		if n > 24 {
			mask += t.Dim(fmt.Sprintf(" … %d chars", n))
		}
		fmt.Fprintf(t.Out, "\r\x1b[K%s%s", prompt, mask)
	}
	draw()
	buf := make([]byte, 4096)
	for {
		k, chars, err := t.readKey(buf)
		if err != nil {
			return "", err
		}
		switch k {
		case keyEnter:
			fmt.Fprint(t.Out, "\r\n")
			return strings.TrimSpace(string(val)), nil
		case keyCancel:
			fmt.Fprint(t.Out, "\r\n")
			return "", ErrCanceled
		case keyBackspace:
			if len(val) > 0 {
				_, size := utf8.DecodeLastRune(val)
				val = val[:len(val)-size]
			}
		case keyChar:
			val = append(val, chars...)
		}
		draw()
	}
}

// Confirm asks a yes/no question (default no).
func (t *Terminal) Confirm(prompt string) (bool, error) {
	i, err := t.Select(prompt, "↑/↓ · enter", []Item{{Label: "No"}, {Label: "Yes"}}, 0)
	return err == nil && i == 1, nil
}
