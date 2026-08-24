require "json"
require "webrick"
require_relative "../../../sdks/ruby/lib/webtest"

raise "this example bridge is test-only" unless ENV["WEBTEST"] == "1"
users = {}
manifest = JSON.parse(File.read(File.join(__dir__, ".webtest/app-schema.json")))
bridge = WebTest::AppBridge.new(manifest)
bridge.register("create_user") do |arguments, _emit|
  email = arguments.fetch("email")
  raise WebTest::ApplicationError.new("email already exists", code: "user.email_taken") if users.key?(email)
  user = { "id" => users.length + 1, "email" => email, "admin" => arguments.fetch("admin", false) }
  users[email] = user
  user
end

server = WEBrick::HTTPServer.new(Port: Integer(ENV.fetch("PORT", "3102")), BindAddress: "127.0.0.1",
                                 Logger: WEBrick::Log.new(File::NULL), AccessLog: [])
server.mount_proc("/health") { |_request, response| response.body = "ok" }
server.mount_proc("/login") do |request, response|
  response["content-type"] = "text/html"
  if request.request_method == "POST"
    email = request.query["email"]
    response.body = users.key?(email) ? "<p>Welcome, #{email}</p>" : "<p>Invalid sign in</p>"
  else
    response.body = '<form method="post"><label>Email <input name="email"></label><button>Sign in</button></form>'
  end
end
Thread.new { server.start }
begin
  bridge.connect_from_env
ensure
  server.shutdown
end

