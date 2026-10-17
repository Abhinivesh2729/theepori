#!/bin/zsh
in=$(cat)
echo "$in" >> "${0:h}/hook.log"
tool=$(echo "$in" | jq -r .tool_name)
cmd=$(echo "$in" | jq -r '.tool_input.command // ""')
if [[ "$tool" == mcp__claude_ai_Atlassian__searchJiraIssuesUsingJql || "$cmd" == *TP_DENY_ME* ]]; then
  echo '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"blocked by tp policy (spike)"}}'
fi
exit 0
