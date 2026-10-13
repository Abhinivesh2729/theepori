package llm

import (
	"bufio"
	"io"
	"strings"
)

// ReadSSE calls fn with the data payload of each server-sent event.
func ReadSSE(r io.Reader, fn func(event, data string) error) error {
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 0, 64*1024), 16*1024*1024)
	var event string
	var data strings.Builder
	flush := func() error {
		if data.Len() == 0 {
			event = ""
			return nil
		}
		d := strings.TrimSuffix(data.String(), "\n")
		data.Reset()
		err := fn(event, d)
		event = ""
		return err
	}
	for sc.Scan() {
		line := sc.Text()
		switch {
		case line == "":
			if err := flush(); err != nil {
				return err
			}
		case strings.HasPrefix(line, ":"):
		case strings.HasPrefix(line, "event:"):
			event = strings.TrimSpace(line[len("event:"):])
		case strings.HasPrefix(line, "data:"):
			data.WriteString(strings.TrimPrefix(line[len("data:"):], " "))
			data.WriteString("\n")
		}
	}
	if err := sc.Err(); err != nil {
		return err
	}
	return flush()
}
