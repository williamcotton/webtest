# Test modeling

`webtest check examples/test-modeling/modeling.webtest` checks labeled cases,
pure case helpers, reusable patterns, soft checks, and semantic preconditions.
`webtest test examples/test-modeling/modeling.webtest --reporter json` runs two
passing variants and one intentionally skipped test. No application or Chrome
session is needed by this example.

For complete reactive browser proof, `cargo test -p webtest --test milestone_h`
starts owned HTTP/websocket fixtures on ephemeral loopback ports and exercises
two actors, websocket JSON selection from a pre-action checkpoint, guards,
case-dependent locators, and public event redaction in real Chrome.

Discover actor/event contracts through `webtest describe language.actor`,
`webtest describe builtin.checkpoint`, `webtest describe control.select`,
`webtest describe language.guard`, and `webtest describe event.network.response`.
