# Repository instructions

Follow the project conventions in [CLAUDE.md](CLAUDE.md).

## Keep documentation in sync

Every change to user-visible behavior, defaults, deployment, security guarantees, protocol formats, or compatibility must update the affected documentation in the same change. Check the README, user guide, technical guide, protocol specification, and HTML manual at `docs/manual/index.html` for affected claims.

Document the actual behavior, applicable versions and platforms, prerequisites, migration effects, and any verification limits. Distinguish released behavior from the working copy, simulation results, and physical verification. For security changes, describe both the protection and the remaining trust boundary. Remove or correct conflicting examples and instructions before declaring the work complete.
