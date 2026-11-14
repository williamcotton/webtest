export function corpus() {
    const source = 'import { identité as make } from "./helpers"\r\ntest "😀é" { let üser = make(value: "a") expect üser == "a" }';
    const path = '/workspace/root.webtest';
    const requests = [];
    const add = (method, params) => requests.push({ protocol: 1, id: requests.length + 1, method, params });
    add('openWorkspace', { configuration: { root: '/workspace', toolchain_identity: 'parity' }, sources: [
        { path: '/workspace/helpers.webtest', text: '/// User identity.\nexport fn identité(value: String) -> String = value', version: 3 },
        { path, text: source, version: 5 },
    ] });
    for (const kind of ['diagnostics', 'compile', 'format', 'document_symbols', 'folding', 'inlay_hints', 'semantic_tokens']) add('query', { kind, path });
    const character = source.split('\n')[1].indexOf('make(value');
    for (const kind of ['completions', 'signature', 'hover', 'definition', 'selection']) add('query', { kind, path, position: { line: 1, character: character + 2 } });
    add('query', { kind: 'references', path, position: { line: 1, character }, include_declaration: true, limit: 100 });
    add('query', { kind: 'rename', path, position: { line: 1, character }, name: 'renamed' });
    add('query', { kind: 'code_actions', path, range: { start: { line: 0, character: 0 }, end: { line: 1, character: 1 } } });
    add('query', { kind: 'project_description', limit: 100 });
    add('query', { kind: 'discovery', limit: 100 });
    add('query', { kind: 'workspace_symbols', query: 'ident', limit: 100 });
    for (const query of ['language.fn', 'control.retry', 'type.String', 'http.get']) add('describe', { query });
    add('openDocument', { path: '/workspace/bad.webtest', text: 'test "bad" { server { let x = http.get( }', version: 1 });
    add('query', { kind: 'diagnostics', path: '/workspace/bad.webtest' });
    add('updateDocument', { path: '/workspace/bad.webtest', version: 2, edits: [{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 39 } }, text: 'test "fixed" {}' }] });
    add('updateDocument', { path, version: 6, edits: [{ range: { start: { line: 1, character: 7 }, end: { line: 1, character: 7 } }, text: 'invalid surrogate' }] });
    add('openDocument', { path: '/workspace/repair.webtest', text: 'test \"repair\" { server { let user: { email: String } = { email: \"a@example.test\" } let value = user.emial } }', version: 1 });
    add('query', { kind: 'diagnostics', path: '/workspace/repair.webtest' });
    const h = '/workspace/h.webtest';
    add('openDocument', { path: h, version: 1, text: `pattern Message(expected: String) = subset {message: expected, token: absent}
guard clean(actor: BrowserActor) {when actor.console.error(e) {fail e.message}}
test "H" cases [case "one" {message: "hello"}, case "two" {message: "again"}] as input {
 actor alice browser
 let mark = checkpoint(alice)
 with guards [clean(alice)] {
  let received = select alice since mark within 1s {when websocket.received_json(e) if e.value matches Message(input.message) {provide e.value} timeout {inconclusive "offline"}}
  check {expect received matches Message(input.message) expect input.message matches regex("^[a-z]+$")}
  alice {expect text(input.message).visible}
 }
}` });
    for (const kind of ['diagnostics', 'compile', 'format', 'document_symbols', 'semantic_tokens']) add('query', { kind, path: h });
    add('query', {kind: 'discovery', limit: 100});
    for (const query of ['language.cases', 'language.pattern', 'pattern.exact', 'language.actor', 'control.select', 'language.guard', 'event.websocket.received_json', 'control.check', 'statement.inconclusive']) add('describe', {query});
    add('compatibility', {});
    add('execute', {}); add('removeFile', { path: '/workspace/helpers.webtest' }); add('query', { kind: 'diagnostics', path });
    return requests;
}
