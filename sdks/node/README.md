# @webtest/node

Protocol-1 application bridge SDK for Node.js/TypeScript. Load the checked-in app manifest,
register each function once, and call `connectFromEnv()` only in the application's explicit test
boot path. The SDK connects outward through `WEBTEST_BRIDGE`, validates arguments/results, keeps logs
off protocol stdout, propagates cancellation through `AbortSignal`, and never parses WebTest source.
Production boot paths must not call `connectFromEnv()`; guard bridge registration with an explicit
test-only flag such as `WEBTEST=1` and fail closed when the flag is absent.

```js
const bridge = new AppBridge(manifest);
bridge.register("create_user", ({ email, admin = false }) => store.create({ email, admin }));
await bridge.connectFromEnv();
```

Run `npm test` and `npm run conformance` from this directory.
