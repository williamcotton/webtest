import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.util.*;
import java.util.concurrent.*;

public class Main {
  static final Map<String, Map<String,Object>> USERS = new ConcurrentHashMap<>();
  static String manifest, schemaHash, functions;
  public static void main(String[] args) throws Exception {
    if (!"1".equals(System.getenv("WEBTEST"))) throw new IllegalStateException("test-only bridge");
    manifest = Files.readString(Path.of(".webtest/app-schema.json"));
    schemaHash = match(manifest, "\\\"schema_hash\\\"\\s*:\\s*\\\"([^\\\"]+)");
    functions = compactJson(objectAfter(manifest, "\"functions\""));
    int port = Integer.parseInt(System.getenv().getOrDefault("PORT", "3106"));
    HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", port), 0);
    server.createContext("/health", exchange -> respond(exchange, "ok", "text/plain"));
    server.createContext("/login", Main::login);
    server.start();
    try { bridge(); } finally { server.stop(0); }
  }
  static void login(HttpExchange exchange) throws IOException {
    if ("POST".equals(exchange.getRequestMethod())) {
      String body = new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8);
      String email = URLDecoder.decode(value(body, "email"), StandardCharsets.UTF_8);
      respond(exchange, USERS.containsKey(email) ? "<p>Welcome, "+email+"</p>" : "<p>Invalid sign in</p>", "text/html");
    } else respond(exchange, "<form method=\"post\"><label>Email <input name=\"email\"></label><button>Sign in</button></form>", "text/html");
  }
  static void bridge() throws Exception {
    URI endpoint = URI.create(System.getenv("WEBTEST_BRIDGE"));
    if (!"tcp".equals(endpoint.getScheme()) || !endpoint.getHost().equals("127.0.0.1")) throw new IllegalStateException("loopback TCP required");
    try (Socket socket = new Socket(endpoint.getHost(), endpoint.getPort());
         BufferedReader in = new BufferedReader(new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8));
         BufferedWriter out = new BufferedWriter(new OutputStreamWriter(socket.getOutputStream(), StandardCharsets.UTF_8))) {
      send(out, "{\"type\":\"hello\",\"protocol_versions\":[1],\"sdk\":\"webtest-java-example\",\"sdk_version\":\"0.1.0\",\"token\":\""+escape(System.getenv("WEBTEST_TOKEN"))+"\",\"capabilities\":{\"cancel\":false,\"events\":false}}");
      if (!in.readLine().contains("\"hello_ok\"")) return;
      String line;
      while ((line = in.readLine()) != null) {
        long id = Long.parseLong(match(line, "\\\"id\\\"\\s*:\\s*(\\d+)"));
        if (line.contains("\"type\":\"describe\"")) send(out, "{\"type\":\"schema\",\"id\":"+id+",\"protocol\":1,\"schema_hash\":\""+schemaHash+"\",\"functions\":"+functions+"}");
        else if (line.contains("\"type\":\"call\"")) {
          String email = unescape(match(line, "\\\"email\\\"\\s*:\\s*\\\"([^\\\"]*)"));
          boolean admin = line.matches(".*\\\"admin\\\"\\s*:\\s*true.*");
          if (USERS.containsKey(email)) send(out, "{\"type\":\"error\",\"id\":"+id+",\"code\":\"user.email_taken\",\"message\":\"email already exists\",\"retryable\":false,\"data\":{}}");
          else { int userId=USERS.size()+1; USERS.put(email, Map.of("id",userId,"email",email,"admin",admin));
            send(out, "{\"type\":\"result\",\"id\":"+id+",\"value\":{\"id\":"+userId+",\"email\":\""+escape(email)+"\",\"admin\":"+admin+"}}"); }
        } else if (line.contains("\"type\":\"ping\"")) send(out, "{\"type\":\"pong\",\"id\":"+id+"}");
        else if (line.contains("\"type\":\"shutdown\"")) { send(out, "{\"type\":\"shutdown_ok\",\"id\":"+id+"}"); return; }
      }
    }
  }
  static void send(BufferedWriter out,String value)throws IOException{out.write(value);out.write("\n");out.flush();}
  static void respond(HttpExchange e,String body,String type)throws IOException{byte[] b=body.getBytes(StandardCharsets.UTF_8);e.getResponseHeaders().set("content-type",type);e.sendResponseHeaders(200,b.length);e.getResponseBody().write(b);e.close();}
  static String value(String query,String key){for(String part:query.split("&")){String[] p=part.split("=",2);if(p[0].equals(key))return p.length>1?p[1]:"";}return "";}
  static String match(String value,String pattern){var m=java.util.regex.Pattern.compile(pattern).matcher(value);if(!m.find())throw new IllegalArgumentException("missing field");return m.group(1);}
  static String objectAfter(String value,String key){int start=value.indexOf('{',value.indexOf(key));int depth=0;boolean string=false,escape=false;for(int i=start;i<value.length();i++){char c=value.charAt(i);if(string){if(escape)escape=false;else if(c=='\\')escape=true;else if(c=='\"')string=false;}else if(c=='\"')string=true;else if(c=='{')depth++;else if(c=='}'&&--depth==0)return value.substring(start,i+1);}throw new IllegalArgumentException("object");}
  static String compactJson(String value){StringBuilder out=new StringBuilder();boolean string=false,escape=false;for(char c:value.toCharArray()){if(string){out.append(c);if(escape)escape=false;else if(c=='\\')escape=true;else if(c=='\"')string=false;}else if(c=='\"'){string=true;out.append(c);}else if(!Character.isWhitespace(c))out.append(c);}return out.toString();}
  static String escape(String v){return v.replace("\\","\\\\").replace("\"","\\\"");}
  static String unescape(String v){return v.replace("\\\"","\"").replace("\\\\","\\");}
}
