require "json"
require_relative "lib/webtest"

manifest = JSON.parse(File.read(File.expand_path("../../protocol/conformance/app-schema.json", __dir__)))
manifest["sdk"] = "webtest-ruby"
manifest["sdk_version"] = "0.1.0"
users = []
bridge = WebTest::AppBridge.new(manifest)
bridge.register("create_user") do |arguments, _emit|
  email = arguments.fetch("email")
  if users.any? { |user| user["email"] == email }
    raise WebTest::ApplicationError.new("a user with that email already exists", code: "user.email_taken")
  end
  user = { "id" => users.length + 1, "email" => email, "admin" => arguments.fetch("admin", false) }
  users << user
  user
end
manifest.fetch("functions").each_key do |name|
  bridge.register(name) { |arguments, _emit| arguments.fetch("value", nil) } if name.start_with?("echo_")
end
bridge.register("wait") do |arguments, emit|
  emit.call("progress", { "phase" => "waiting" })
  sleep(arguments.fetch("delay_ms") / 1000.0)
  "completed"
end
bridge.connect_from_env
