#!/usr/bin/env bash
# Verify that the 6 split skills together contain the same substantive lines
# as the monolithic .agents/instructions/rootcx-sdk.md source.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="$ROOT/.agents/instructions/rootcx-sdk.md"
SKILLS="$ROOT/.agents/skills"

# Strip frontmatter blocks and blank lines, keep only code/text with content.
norm() { awk 'BEGIN{fm=0} /^---$/{fm=1-fm; next} !fm && NF' "$@" | sort -u; }

SRC_LINES=$(norm "$SRC")
SPLIT_LINES=$(norm \
  "$SKILLS"/rootcx-manifest/SKILL.md \
  "$SKILLS"/rootcx-sdk-hooks/SKILL.md \
  "$SKILLS"/rootcx-ui/SKILL.md \
  "$SKILLS"/rootcx-ui/references/components.md \
  "$SKILLS"/rootcx-backend-worker/SKILL.md \
  "$SKILLS"/rootcx-rest-api/SKILL.md \
  "$SKILLS"/rootcx-rest-api/references/collections.md \
  "$SKILLS"/rootcx-rest-api/references/integrations.md \
  "$SKILLS"/rootcx-rest-api/references/jobs.md \
  "$SKILLS"/rootcx-agent/SKILL.md)

missing=$(comm -23 <(echo "$SRC_LINES") <(echo "$SPLIT_LINES") || true)
if [ -n "$missing" ]; then
  echo "✗ lines present in source but missing from split skills:"
  echo "$missing"
  exit 1
fi
echo "✓ all substantive lines from rootcx-sdk.md are present in the split skills"
