#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
    if command -v curl >/dev/null 2>&1; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
            sh -s -- -y --profile minimal
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- https://sh.rustup.rs | sh -s -- -y --profile minimal
    else
        printf '%s\n' "cargo is missing and rustup requires curl or wget" >&2
        exit 1
    fi

    if [[ -f "$HOME/.cargo/env" ]]; then
        # rustup updates this file when the toolchain location changes.
        source "$HOME/.cargo/env"
    fi
fi

if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' "cargo is unavailable after rustup installation" >&2
    exit 1
fi

cargo install --path "$script_dir"

skill_src="$script_dir/skills/trv"
if [[ ! -f "$skill_src/SKILL.md" ]]; then
    printf '%s\n' "agent skill is missing at $skill_src/SKILL.md" >&2
    exit 1
fi

# Vendor-neutral Agent Skills path plus the common coding-agent homes, so a
# freshly installed trv is already the review workflow for Grok, Claude Code,
# Codex, Cursor, Gemini, Copilot, and anything else that reads SKILL.md.
skill_dests=(
    "$HOME/.agents/skills/trv"
    "$HOME/.grok/skills/trv"
    "$HOME/.claude/skills/trv"
    "$HOME/.codex/skills/trv"
    "$HOME/.cursor/skills/trv"
    "$HOME/.gemini/skills/trv"
    "$HOME/.copilot/skills/trv"
)

for dest in "${skill_dests[@]}"; do
    mkdir -p "$(dirname "$dest")"
    rm -rf "$dest"
    mkdir -p "$dest"
    cp "$skill_src/SKILL.md" "$dest/SKILL.md"
done

# Replaced by skills/trv when the in-session workflow landed.
rm -rf "$HOME/.grok/skills/trv-agent"
