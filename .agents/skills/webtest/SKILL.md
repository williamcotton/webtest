---
name: webtest
description: Discover, inspect, author, validate, and run WebTest web-system tests using native CLI capabilities (`describe`, `inspect`, `check`, `test`, `fmt`).
---

# WebTest Authoring & Discovery

WebTest is a statically analyzable language for web-system tests. The language is **self-describing** and **live-inspectable**. To avoid wasting context tokens, perform targeted, surgical CLI queries for specific constructs rather than dumping the full language surface.

## 1. Surgical Discovery (`webtest describe`)

Perform targeted queries for specific constructs, providers, or application bridge specifications:

- **Targeted Construct Query** (returns exact syntax, parameter types, legal contexts, and canonical examples):
  `webtest describe locator.role`
  `webtest describe browser.click`
  `webtest describe provider.http.post`
- **Application Bridge & Protocol Query** (returns full Protocol 1 wire spec, JSON framing, transport scheme/retries, offline schema manifest, and single-source-of-truth implementation pseudocode):
  `webtest describe app`
  `webtest describe app.pseudocode`
  `webtest describe app.create_user` (in a project with app-schema)
- **Search Capabilities**:
  `webtest describe --search "json post"`
- **Machine Format**:
  `webtest describe <query> --reporter json`

## 2. Live Page Inspection (`webtest inspect`)

Discover validated semantic locators on live web pages without raw DOM scraping:

- **Inspect page**:
  `webtest inspect http://127.0.0.1:3000/login --reporter json`

## 3. Test Structure Anatomy

```webtest
test "user authentication flow" {
    server {
        let user = app.create_user(email: "alice@example.com", admin: false)
        let res = http.post("/api/login", json: { email: user.email })
        expect res.status == 200
    }

    browser {
        open "/login"
        fill label("Email") with user.email
        click role("button", name: "Sign in")
        expect text("Welcome").visible
    }
}
```

## 4. Closed-Loop Authoring Workflow

1. **Inspect Target Page**: Run `webtest inspect <url>` for canonical locators.
2. **Surgically Discover Vocabulary**: Run `webtest describe <query>` (e.g. `webtest describe app` or `webtest describe app.pseudocode`) for specific parameter signatures, wire protocol specs, or examples.
3. **Format Code**: Run `webtest fmt <path>` to canonicalize formatting.
4. **Statically Check**: Run `webtest check <path> --reporter json`. Diagnostics contain `reference_queries` (e.g. `locator.role`) pointing to the exact `webtest describe` topic needed for repair.
5. **Execute Test**: Run `webtest test <path> [--headed]`. Structured failure outputs provide `repair_hints` with replacement candidates.

## 5. Canonical `webtest.toml` Configuration

```toml
[browser]
base_url = "http://127.0.0.1:3000"
headless = true

[server]
base_url = "http://127.0.0.1:3000"

[server.app]
schema = ".webtest/app-schema.json"

[app]
command = "npm"
args = ["run", "dev"]
```
