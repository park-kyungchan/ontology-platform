# Forward correction: strict judge values (independent review required)

The preimplementation v1 judge at 430d186 uses Python `==` on parsed JSON.
`True == 1` and duplicate JSON keys can therefore pass its comparison.
This is a comparator defect, not a changed product expectation. No Rust candidate
has been run or used as evidence. v1 files/pins are retained unchanged.

`verify_v2.py` adds recursive exact type equality, duplicate-key refusal and one
compact JSON frame (no leading whitespace or extra frames/lines), plus
actual Python children with plausible JSON/exit 7 and a wrong crash status. It
reuses every unchanged v1 literal case. Lead must obtain independent review of
this correction before using candidate results for acceptance. No implementer
self-check is an independent review. New pins cover v2 separately.
