# CLAUDE.md

## CRITICAL: Serena MCP Availability — HARD REQUIREMENT

**If Serena MCP is unavailable, STOP IMMEDIATELY. Do NOT continue any work. Do NOT attempt workarounds.**
Serena is MANDATORY for this project. There are ZERO exceptions. If Serena tools cannot be reached, inform the user and halt ALL operations until Serena is restored. No code may be written, no files may be created, no tasks may proceed without Serena.

## MANDATORY: Serena Memory System — Full Context Loading

This project uses **Serena MCP memories** as the single source of truth for ALL development rules, conventions, and project knowledge.

**At the start of EVERY session, execute the following steps IN ORDER. ALL steps are MANDATORY. NONE may be skipped.**

1. Call `check_onboarding_performed` — NO EXCEPTION
2. Call `initial_instructions` if not yet read in this session
3. Call `list_memories` to discover all available memories
4. **Read `global_` memories using the smart loading policy below.**
5. **Read all `emusphere/` and `project_` memories that are contextually relevant to the current task.**
6. Only after all required memories are loaded: begin working on the user's task

**This is non-negotiable. The full applicable rule set must be loaded before any work begins.**

**Smart loading policy for `global_` memories:**

- **ALWAYS read** (language-agnostic, apply to every project):
  - `global_coding_rules`
  - `global_context_maintenance`
  - `global_refactoring_rules`
  - `global_git_conventions`
  - `global_ferroscope_best_practices` (if debugger MCP is available)
  - `global_project_init` (only during onboarding)
- **Read ONLY if matching project tech stack**:
  - `global_coding_rules_rust` - this project: YES
  - `global_coding_rules_python`, `global_coding_rules_java`, `global_coding_rules_js_svelte` - this project: NO (skip)
- **Read ONLY if applicable to project type**:
  - `global_fk_naming_convention` - only if project uses a relational database (this project: NO)
  - `global_security_rules` - only if project is a web application (this project: NO)
  - `global_diagnostics_subsystem_rules` - only if project has a diagnostics subsystem (this project: YES)

**Memory naming convention:**

- `global_` - Generic rules, loaded per smart policy above
- `*/*.md` - Project-specific knowledge for this emulator
- `project_` - Project-specific knowledge (legacy prefix, same as above)

**All rules, conventions, architecture details, and commands live in Serena memories.**

Do NOT add rules to this file - create or update Serena memories instead.

FINALLY : NEVER TRUST COMPACTION REPORT !
