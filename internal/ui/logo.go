package ui

import (
	"fmt"
	"os"
	"strings"
	"unicode"

	"golang.org/x/term"
)

// flame is drawn from a mask: '#' outer flame, '*' hot core.
var flame = []string{
	"       #     ",
	"       ##    ",
	"      ###    ",
	"      ####  #",
	"     ##### ##",
	"    #########",
	"   ####*#### ",
	"  ####***####",
	"  ###*****###",
	"  ###*****###",
	"   ###***### ",
	"    #######  ",
	"      ###    ",
}

// 256-colour palette: tips red → body orange → base amber; core yellow.
var (
	flameRows = []int{196, 196, 202, 202, 202, 208, 208, 208, 214, 214, 214, 220, 220}
	coreColor = 226
	nameHues  = []int{226, 220, 208, 196} // தீ · ப் · பொ · றி
)

const tamilName = "தீப்பொறி"

// Clusters splits text into user-perceived characters: a base letter plus
// its combining marks (Tamil vowel signs and pulli). Colouring per cluster
// keeps marks attached to their consonant.
func Clusters(s string) []string {
	var out []string
	for _, r := range s {
		if len(out) > 0 && (unicode.Is(unicode.Mn, r) || unicode.Is(unicode.Mc, r)) {
			out[len(out)-1] += string(r)
			continue
		}
		out = append(out, string(r))
	}
	return out
}

func (c *Console) fg(code int, s string) string {
	if !c.Color {
		return s
	}
	return fmt.Sprintf("\x1b[38;5;%dm%s\x1b[0m", code, s)
}

// TamilName renders தீப்பொறி with one fire colour per letter.
func (c *Console) TamilName() string {
	var b strings.Builder
	for i, cl := range Clusters(tamilName) {
		b.WriteString(c.fg(nameHues[i%len(nameHues)], cl))
	}
	return c.bold(b.String())
}

// Logo returns the flame with the name beside it, or a single line when the
// terminal is narrow or TP_NO_LOGO is set.
func (c *Console) Logo(version string) string {
	width := 80
	if w, _, err := term.GetSize(int(os.Stdout.Fd())); err == nil && w > 0 {
		width = w
	}
	if os.Getenv("TP_NO_LOGO") != "" || width < 44 {
		return c.TamilName() + " " + c.bold("tp") + " " + c.dim(version) + "\n"
	}
	side := map[int]string{
		6: c.TamilName(),
		7: c.bold("Thee Pori") + c.dim(" · tp "+version),
		8: c.dim("the spark for your code"),
	}
	var b strings.Builder
	for i, row := range flame {
		var line strings.Builder
		for _, ch := range row {
			switch ch {
			case '#':
				line.WriteString(c.fg(flameRows[i], "█"))
			case '*':
				if c.Color {
					line.WriteString(c.fg(coreColor, "█"))
				} else {
					line.WriteString("▒") // keep the core visible without colour
				}
			default:
				line.WriteByte(' ')
			}
		}
		b.WriteString("  " + strings.TrimRight(line.String(), " "))
		if s, ok := side[i]; ok {
			b.WriteString(strings.Repeat(" ", len(row)-len(strings.TrimRight(row, " "))) + "    " + s)
		}
		b.WriteString("\n")
	}
	return b.String()
}
