using System.Collections.Concurrent;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

if (Environment.GetEnvironmentVariable("WEBTEST") != "1") throw new Exception("test-only bridge");
var users = new ConcurrentDictionary<string, JsonObject>();
var manifest = JsonNode.Parse(await File.ReadAllTextAsync(".webtest/app-schema.json"))!.AsObject();
var listener = new HttpListener();
var port = Environment.GetEnvironmentVariable("PORT") ?? "3107";
listener.Prefixes.Add($"http://127.0.0.1:{port}/");
listener.Start();
var httpTask = Task.Run(async () => {
  while (listener.IsListening) {
    HttpListenerContext context;
    try { context = await listener.GetContextAsync(); } catch { break; }
    var request = context.Request; string body;
    if (request.Url!.AbsolutePath == "/health") body = "ok";
    else if (request.Url.AbsolutePath == "/login" && request.HttpMethod == "POST") {
      using var reader = new StreamReader(request.InputStream, request.ContentEncoding);
      var form = WebUtility.UrlDecode((await reader.ReadToEndAsync()).Split('=').Last().Replace("+", " "));
      body = users.ContainsKey(form) ? $"<p>Welcome, {form}</p>" : "<p>Invalid sign in</p>";
    } else if (request.Url.AbsolutePath == "/login") body = "<form method=\"post\"><label>Email <input name=\"email\"></label><button>Sign in</button></form>";
    else { context.Response.StatusCode=404; body="not found"; }
    context.Response.ContentType = request.Url.AbsolutePath == "/health" ? "text/plain" : "text/html";
    var bytes=Encoding.UTF8.GetBytes(body); context.Response.ContentLength64=bytes.Length; await context.Response.OutputStream.WriteAsync(bytes); context.Response.Close();
  }
});
try {
  var endpoint = new Uri(Environment.GetEnvironmentVariable("WEBTEST_BRIDGE")!);
  if (endpoint.Scheme != "tcp" || endpoint.Host != "127.0.0.1") throw new Exception("loopback TCP required");
  using var client = new TcpClient(); await client.ConnectAsync(endpoint.Host, endpoint.Port);
  await using var stream=client.GetStream(); var utf8=new UTF8Encoding(false); using var input=new StreamReader(stream,utf8,leaveOpen:true); await using var output=new StreamWriter(stream,utf8,leaveOpen:true){AutoFlush=true};
  async Task Send(JsonObject value) => await output.WriteLineAsync(value.ToJsonString());
  await Send(new JsonObject{{"type","hello"},{"protocol_versions",new JsonArray(1)},{"sdk","webtest-dotnet-example"},{"sdk_version","0.1.0"},{"token",Environment.GetEnvironmentVariable("WEBTEST_TOKEN")},{"capabilities",new JsonObject{{"cancel",false},{"events",false}}}});
  if (JsonNode.Parse((await input.ReadLineAsync())!)!["type"]!.GetValue<string>() != "hello_ok") return;
  string? line;
  while ((line=await input.ReadLineAsync()) is not null) {
    var message=JsonNode.Parse(line)!.AsObject(); var kind=message["type"]!.GetValue<string>(); var id=message["id"]!.GetValue<long>();
    if(kind=="describe") await Send(new JsonObject{{"type","schema"},{"id",id},{"protocol",1},{"schema_hash",manifest["schema_hash"]!.DeepClone()},{"functions",manifest["functions"]!.DeepClone()}});
    else if(kind=="call") {
      var argumentsValue=message["arguments"]!.AsObject(); var email=argumentsValue["email"]!.GetValue<string>(); var admin=argumentsValue["admin"]?.GetValue<bool>()??false;
      if(users.ContainsKey(email)) await Send(new JsonObject{{"type","error"},{"id",id},{"code","user.email_taken"},{"message","email already exists"},{"retryable",false},{"data",new JsonObject()}});
      else { var user=new JsonObject{{"id",users.Count+1},{"email",email},{"admin",admin}};users[email]=user;await Send(new JsonObject{{"type","result"},{"id",id},{"value",user.DeepClone()}}); }
    } else if(kind=="ping") await Send(new JsonObject{{"type","pong"},{"id",id}});
    else if(kind=="shutdown"){await Send(new JsonObject{{"type","shutdown_ok"},{"id",id}});break;}
  }
} finally { listener.Stop(); await httpTask; }
