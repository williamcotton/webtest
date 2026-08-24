# webtest Ruby SDK

Protocol-1 application bridge SDK for Ruby. It loads the checked-in offline manifest, rejects
duplicate/undeclared registrations, validates arguments and results, supports Unix/loopback TCP and
stdio framing, converts bounded application errors, and keeps logs away from protocol stdout.
Only invoke `connect_from_env` behind an explicit test-only boot flag such as `WEBTEST=1`; production
startup must leave the privileged fixture bridge disabled.

```ruby
bridge = WebTest::AppBridge.new(manifest)
bridge.register("create_user") { |args, _emit| Store.create(args) }
bridge.connect_from_env
```

Run `ruby test_sdk.rb` and
`python3 ../../protocol/conformance/harness.py ruby conformance.rb` from this directory.
