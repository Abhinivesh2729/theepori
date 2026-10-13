package permission

import (
	"regexp"
	"strings"
)

type rule struct {
	re     *regexp.Regexp
	reason string
}

var destructive = []rule{
	{regexp.MustCompile(`\brm\s+(-[A-Za-z]*[rRf][A-Za-z]*\s+)+`), "recursive/forced delete"},
	{regexp.MustCompile(`\bgit\s+push\b`), "publishes commits to a remote"},
	{regexp.MustCompile(`\bgit\s+reset\s+--hard\b`), "discards uncommitted work"},
	{regexp.MustCompile(`\bgit\s+clean\s+-[A-Za-z]*f`), "deletes untracked files"},
	{regexp.MustCompile(`\bgit\s+(checkout|restore)\s+(--\s+)?\.(\s|$)`), "discards working-tree changes"},
	{regexp.MustCompile(`\bgit\s+branch\s+-D\b`), "force-deletes a branch"},
	{regexp.MustCompile(`\bgit\s+(rebase|filter-branch|filter-repo)\b`), "rewrites history"},
	{regexp.MustCompile(`\bgit\s+stash\s+(drop|clear)\b`), "deletes stashed work"},
	{regexp.MustCompile(`\bsudo\b`), "runs as root"},
	{regexp.MustCompile(`\b(mkfs|fdisk|diskutil\s+erase)`), "formats disks"},
	{regexp.MustCompile(`\bdd\s+.*\bof=`), "raw disk write"},
	{regexp.MustCompile(`\b(chmod|chown)\s+-R\b`), "recursive permission change"},
	{regexp.MustCompile(`\b(shutdown|reboot|halt)\b`), "stops the machine"},
	{regexp.MustCompile(`(curl|wget)\b[^|]*\|\s*(sudo\s+)?(ba|z|fi)?sh\b`), "executes a downloaded script"},
	{regexp.MustCompile(`\bkubectl\s+(delete|apply|replace|scale|drain)\b`), "changes a cluster"},
	{regexp.MustCompile(`\bterraform\s+(apply|destroy|import)\b`), "changes infrastructure"},
	{regexp.MustCompile(`\bpulumi\s+(up|destroy)\b`), "changes infrastructure"},
	{regexp.MustCompile(`\bhelm\s+(install|upgrade|uninstall|delete|rollback)\b`), "changes a cluster"},
	{regexp.MustCompile(`\b(vercel|netlify)\b.*(--prod|deploy)`), "deploys"},
	{regexp.MustCompile(`\bwrangler\s+(deploy|publish|delete)\b`), "deploys to Cloudflare"},
	{regexp.MustCompile(`\bfirebase\s+(deploy|.*:delete)`), "deploys to Firebase"},
	{regexp.MustCompile(`\baws\s+\S+\s+(delete|terminate|rm|remove|destroy|put-bucket-policy)`), "changes AWS resources"},
	{regexp.MustCompile(`\bgcloud\b.*\b(delete|deploy)\b`), "changes GCP resources"},
	{regexp.MustCompile(`\bdocker\s+(system\s+prune|volume\s+(rm|prune)|rm|rmi)\b`), "deletes containers/images/volumes"},
	{regexp.MustCompile(`(?i)\b(drop\s+(table|database|schema)|truncate\s+table)\b`), "destroys database data"},
	{regexp.MustCompile(`\b(npm|pnpm|yarn)\s+publish\b|\bcargo\s+publish\b|\btwine\s+upload\b`), "publishes a package"},
	{regexp.MustCompile(`\bgh\s+(pr\s+(merge|close)|release\s+(create|delete)|repo\s+(delete|edit)|secret)\b`), "changes GitHub state"},
	{regexp.MustCompile(`\bfind\b.*\s-delete\b`), "bulk delete"},
	{regexp.MustCompile(`\bxargs\s+(-\S+\s+)*rm\b`), "bulk delete"},
	{regexp.MustCompile(`>\s*/dev/(sd|disk|nvme)`), "raw disk write"},
	{regexp.MustCompile(`:\(\)\s*\{`), "fork bomb"},
	{regexp.MustCompile(`\b(ssh-keygen|gpg\s+--delete|security\s+delete)`), "credential change"},
	{regexp.MustCompile(`\b(export|setx)\s+\w*(KEY|TOKEN|SECRET|PASSWORD)\w*=`), "credential change"},
}

var readOnlyCmds = map[string]bool{
	"ls": true, "pwd": true, "cat": true, "head": true, "tail": true, "wc": true, "grep": true,
	"rg": true, "echo": true, "which": true, "file": true, "stat": true, "du": true, "df": true,
	"tree": true, "date": true, "whoami": true, "uname": true, "env": false, "printenv": false,
	"sort": true, "uniq": true, "cut": true, "diff": true, "basename": true, "dirname": true, "realpath": true,
}

var readOnlyGit = map[string]bool{
	"status": true, "diff": true, "log": true, "show": true, "rev-parse": true, "ls-files": true,
	"blame": true, "branch": true, "remote": true, "describe": true, "shortlog": true, "tag": true,
}

// ClassifyCommand returns the risk of a shell command and, for destructive
// commands, the reason.
func ClassifyCommand(cmd string) (Risk, string) {
	for _, r := range destructive {
		if r.re.MatchString(cmd) {
			return RiskDestructive, r.reason
		}
	}
	if isReadOnly(cmd) {
		return RiskRead, ""
	}
	return RiskExec, ""
}

var splitRe = regexp.MustCompile(`\|\||&&|[|;\n]`)

func isReadOnly(cmd string) bool {
	if strings.ContainsAny(cmd, ">`") || strings.Contains(cmd, "$(") || strings.Contains(cmd, "<(") {
		return false
	}
	for _, seg := range splitRe.Split(cmd, -1) {
		f := strings.Fields(seg)
		if len(f) == 0 {
			continue
		}
		switch {
		case readOnlyCmds[f[0]]:
		case f[0] == "find":
			if strings.Contains(seg, "-exec") || strings.Contains(seg, "-delete") || strings.Contains(seg, "-ok") {
				return false
			}
		case f[0] == "git" && len(f) > 1 && readOnlyGit[f[1]]:
			if f[1] == "branch" && len(f) > 2 || f[1] == "tag" && len(f) > 2 || f[1] == "remote" && len(f) > 2 && f[2] != "-v" {
				return false
			}
		default:
			return false
		}
	}
	return true
}

// CommandKey is the approval-memory key for "always allow": the program
// plus its subcommand (e.g. "go test", "npm run").
func CommandKey(cmd string) string {
	f := strings.Fields(cmd)
	switch {
	case len(f) == 0:
		return "exec:"
	case len(f) >= 2 && !strings.HasPrefix(f[1], "-"):
		return "exec:" + f[0] + " " + f[1]
	default:
		return "exec:" + f[0]
	}
}
