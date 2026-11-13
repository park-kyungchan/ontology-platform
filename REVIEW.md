# Review policy of ontology-platform

For the one reviewer session of a pull request (and for the automatic reviewer). Review the head whose required check `rust` passed.

## A finding blocks only when it is one of these, shown by a command or a test you supply

1. Wrong behaviour: a sequence of commands of this repository's binaries, on a store they produced, after which a read omits or wrongly returns a head or a count, an action is applied or refused against its criteria, or a command's answer contradicts `crates/ont/README.md`.
2. Loss or exposure of data: a write, a migration, `compact`, `backup` or an interrupted run that loses or damages rows; a reader that learns of objects outside its grants.
3. A broken contract: the schema language, an answer's shape, the store format or a public library item changes without the pull request saying so, or an installed schema no longer installs.
4. Unbounded work: the work of one read or one action grows without a stated bound.
5. An irreversible effect on a real target without a guard (the install script, a store upgrade).

## Not blocking (at most five notes)

A test gap, a surviving mutant, a sentence in a document, a name, a preference for another structure, hardening against a same-user adversary. Do not write mutants by hand. A change that deletes the only tests of code that stays says so in the pull request; when it does not, that is a finding of kind 3.

## Form

First line exactly `Verdict: accept` or `Verdict: repair`; second line the head revision; then the blocking findings (each with file, line and the command or test), then the notes.
