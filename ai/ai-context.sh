#!/bin/bash

# ai-context.sh - Bundles project context for AI assistants

set -e

# Output file
CONTEXT_FILE="./ai/CONTEXT_BUNDLE.md"

echo "Generating AI context bundle..."

{
    echo "# AI Context Bundle"
    echo "Generated: $(date)"
    echo ""
    echo "## ⚠️ Agent Navigation Guide"
    echo "1. Start with the **Current State** below to understand the focus."
    echo "2. Check **Active Tasks** for your specific assignment."
    echo "3. Only read files from the repository structure that are directly related to those tasks."
    echo "4. Do NOT perform full repository scans unless the task is an architectural audit."
    echo ""
    
    echo "## 1. Authoritative Rules (AGENTS.md)"
    if [ -f "ai/AGENTS.md" ]; then
        cat "ai/AGENTS.md"
    else
        echo "Warning: ai/AGENTS.md not found."
    fi
    echo ""

    echo "## 2. Architecture (ARCHITECTURE.md)"
    if [ -f "ai/ARCHITECTURE.md" ]; then
        cat "ai/ARCHITECTURE.md"
    else
        echo "Warning: ai/ARCHITECTURE.md not found."
    fi
    echo ""

    echo "## 3. Project State (PROJECT_STATE.md)"
    if [ -f "ai/PROJECT_STATE.md" ]; then
        cat "ai/PROJECT_STATE.md"
    else
        echo "Warning: ai/PROJECT_STATE.md not found."
    fi
    echo ""

    echo "## 4. Repository Structure"
    echo "\`\`\`text"
    find . -maxdepth 2 -not -path '*/.*'
    echo "\`\`\`"
    echo ""

    echo "## 5. Recent Git Changes (Summary)"
    echo "\`\`\`text"
    git log -n 5 --oneline || echo "No git history yet."
    echo "\`\`\`"
    echo ""
    
    echo "## 6. Active Diff"
    echo "\`\`\`diff"
    git diff --cached | head -n 100
    git diff | head -n 100
    echo "\`\`\`"

} > "$CONTEXT_FILE"

echo "✅ Context bundle created at $CONTEXT_FILE"
echo "Tokens estimated: $(wc -w < "$CONTEXT_FILE") words."
