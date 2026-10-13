// Command tp is the Thee Pori (தீப்பொறி) coding agent CLI.
package main

import (
	"os"

	"github.com/Abhinivesh2729/theepori/internal/cli"
)

func main() {
	os.Exit(cli.Execute())
}
