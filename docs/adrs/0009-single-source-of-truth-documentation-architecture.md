# ADR 0009: Global Single Source of Truth (SSOT) Documentation Architecture and Domain Indexing

Status: Implemented

## Context

As the LicoUp codebase and documentation evolved, technical specifications and subsystem details—such as the 4-tier architectural models, Canonical Conversation domain structures, endpoint security and encryption specifications, subagent wire protocols, Adaptive Flywheel strategies, platform-specific OS adaptations, and model token-cost metadata catalogs—were repeatedly duplicated and aggregated into top-level overview files or across multiple peer documents.

This monolithic and fragmented approach introduced severe structural issues:
1. **Compounding Documentation Drift**: When subsystem rules, wire formats, or state machines evolved, changes in domain files were not consistently synchronized with top-level architecture summaries, causing contradictory claims.
2. **Monolithic Bloat & Blurred Abstraction**: Top-level overview documents (architecture, protocols, functionalities) became cluttered with low-level implementation details, obscuring the boundary between global system abstractions and subsystem mechanisms.
3. **Absence of a Single Source of Truth (SSOT)**: Human developers and automated boundary verification scripts could not reliably determine which document was the ultimate authority for a given feature or lifecycle state.

## Decision

All technical and product documentation in this repository (encompassing architecture, protocols, functionality, platforms, operations, and contributing standards) must strictly adhere to the following authoring and organizational rules:

1. **Global Single Source of Truth (SSOT)**:
   - Across the entire repository, every architectural model, domain concept, wire protocol format, lifecycle state machine, security/encryption rule, or platform capability **must have exactly one authoritative owning document**.
   - Any other document referencing that concept **must link to or tabulate that authoritative owner; re-stating, paraphrasing, or copy-pasting facts is strictly forbidden**.
2. **Top-Level & Overview Document Modularity**:
   - Top-level overview documents (e.g., `docs/architecture/README.md`, `docs/protocols/README.md`, `docs/functionality/README.md`) must remain concise and high-level, documenting only cross-cutting tiers, flowcharts, and universal boundaries.
   - Subsystem and domain-specific details must reside in dedicated domain documents, linked from the overview documents via **Structured Index Tables**.
3. **Standardized Index Tables and Cross-Reference Headers**:
   - **Header References**: Every document must start with a clean Markdown table listing normative English versions, localizations, and governing product/status definitions with explicit authority declarations.
   - **Domain Architecture / Protocol Index Tables**: Overview documents must provide structured tables declaring Domain Name, Owning Tier/Category, Authoritative Document Path, and Core Responsibilities.
4. **Separation of Product Philosophy and Technical Facts**:
   - Product vision, design philosophies (Diverse, Connected, Open, Integrated), and top-level user promises belong exclusively to `PRODUCT.md` / `PRODUCT.zh-CN.md`.
   - Technical architecture and protocol documents focus purely on technical facts, invariants, interface boundaries, and code mappings.
5. **Strict Bilingual Synchronization**:
   - English documentation is the normative authority for shared technical facts. Simplified Chinese localizations must link back to their English counterpart, and technical facts must remain strictly in sync.

## Documentation SSOT Ownership Matrix

| Documentation Category | Directory / File | Authoritative Ownership Scope | Authoring Standard |
|:---|:---|:---|:---|
| **Product Goals & Philosophy** | `PRODUCT.md` | Product vision, design philosophy, durable promises, command identity | Top-level governing charter; avoids low-level code details |
| **Top-Level System Architecture** | `docs/architecture/README.md` | 4-tier client architecture, cross-cutting flowcharts, and Native OS adaptation boundaries | Avoids subsystem details; navigates via domain index tables |
| **Domain Architecture Specifications** | `docs/architecture/*.md` | Conversation domain model, security & data boundaries, client-native bridging contracts | Owns the unique models, state machines, and entity definitions for that domain |
| **Feature Specifications** | `docs/functionality/*.md` | Adaptive Flywheel strategies, desktop client capability boundaries, user guides, design systems | Focuses on standalone feature mechanisms and lifecycles |
| **Wire Protocols & Schemas** | `docs/protocols/*.md` | Subagent MCP, Conversation MCP, Lico Agent, LLM Gateway, Station Adapter | Strict wire fields, envelope formats, RPC exchanges, and codecs |
| **Platform Compliance** | `docs/platforms/*.md` | macOS direct-distribution compliance, OS-specific requirements | Non-portable OS specifications and release compliance checklists |
| **Current Status & Compatibility** | `docs/STATUS.md`, `COMPATIBILITY.md` | Verified implementation facts, platform and 13-agent support matrices | Sole truth for current support; never claim unverified capabilities |

## Consequences

- **Eliminates Documentation Drift**: Any technical change requires updating only its single authoritative owner document.
- **Clear Navigation & Discoverability**: Developers and reviewers navigate through structured tables from high-level overviews to deep domain specs.
- **Enables Automated CI Audits**: Architecture gate scripts can verify subsystem boundaries directly against SSOT declarations without parsing ambiguous duplicate summaries.

## Implementation Status

- `docs/architecture/` refactored into the 4-tier overview + dedicated domain documents (`CONVERSATION-DOMAIN.md`, `SECURITY-AND-DATA-BOUNDARY.md`, `CLIENT-NATIVE-INTERACTION.md`).
- `PRODUCT.md` / `PRODUCT.zh-CN.md` updated with the consolidated Design Philosophy.
- `CONTRIBUTING.md` / `CONTRIBUTING.zh-CN.md` updated with explicit SSOT documentation authoring rules.
