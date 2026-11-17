# Project rules

## Language: English only

Everything written from now on must be in English:

- code comments and doc comments
- identifiers (functions, types, variables, modules)
- log messages, error messages and CLI help
- UI text in the web client, Android app and firmware display
- CLI flags, data file and folder names
- documentation (README, docs/, compare.md, reports)
- commit messages and PR descriptions
- test names and test output

Existing Danish text is left as it is until the planned translation job
converts the whole repository, including file names, CLI flags and data
files. Do not mix languages inside a new function or file: new code next to
Danish code is still written in English.

## Commits

Do not add `Co-Authored-By: Claude` or any other Claude attribution
(Claude-Session line, "Generated with Claude Code") to commit messages or
PR descriptions.

## Android app and PC web client stay level

Every user-facing change in the Android app must also be made in the PC
host's web client (maarse-client, served by maarse-host), and the other way
round, in the same piece of work, without being asked. Before reporting a
feature as done, check both; say plainly if one side is not done.
