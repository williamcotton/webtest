if System.get_env("WEBTEST") != "1", do: raise("test-only bridge")
{:ok, users} = Agent.start_link(fn -> %{} end)
manifest = File.read!(".webtest/app-schema.json")
[_, schema_hash] = Regex.run(~r/"schema_hash"\s*:\s*"([^"]+)"/, manifest)
[_, functions] = Regex.run(~r/"functions"\s*:\s*(\{.*\})\s*\}\s*$/s, manifest)
functions = Regex.replace(~r/\s+(?=(?:[^\"]*\"[^\"]*\")*[^\"]*$)/, functions, "")

send_frame = fn socket, value -> :gen_tcp.send(socket, value <> "\n") end

recv_request = fn recv_request, socket, data ->
  case String.split(data, "\r\n\r\n", parts: 2) do
    [headers, body] ->
      content_length =
        case Regex.run(~r/^content-length:\s*(\d+)\r?$/im, headers) do
          [_, length] -> String.to_integer(length)
          _ -> 0
        end

      if byte_size(body) >= content_length do
        data
      else
        {:ok, chunk} = :gen_tcp.recv(socket, 0, 2_000)
        recv_request.(recv_request, socket, data <> chunk)
      end

    _ ->
      {:ok, chunk} = :gen_tcp.recv(socket, 0, 2_000)
      recv_request.(recv_request, socket, data <> chunk)
  end
end

http_loop = fn loop, listener ->
  {:ok, socket} = :gen_tcp.accept(listener)
  request = recv_request.(recv_request, socket, "")
  [line | _] = String.split(request, "\r\n")
  content =
    cond do
      String.starts_with?(line, "GET /health ") -> "ok"
      String.starts_with?(line, "GET /login ") ->
        ~s(<form method="post"><label>Email <input name="email"></label><button>Sign in</button></form>)
      String.starts_with?(line, "POST /login ") ->
        body = request |> String.split("\r\n\r\n", parts: 2) |> List.last()
        email = body |> String.replace_prefix("email=", "") |> String.replace("+", " ") |> URI.decode_www_form()
        if Agent.get(users, &Map.has_key?(&1, email)), do: "<p>Welcome, #{email}</p>", else: "<p>Invalid sign in</p>"
      true -> "not found"
    end
  response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: #{byte_size(content)}\r\nConnection: close\r\n\r\n#{content}"
  :gen_tcp.send(socket, response)
  :gen_tcp.close(socket)
  loop.(loop, listener)
end
{:ok, listener} = :gen_tcp.listen(String.to_integer(System.get_env("PORT") || "3105"), [:binary, active: false, reuseaddr: true, ip: {127,0,0,1}])
Task.start(fn -> http_loop.(http_loop, listener) end)

endpoint = URI.parse(System.fetch_env!("WEBTEST_BRIDGE"))
if endpoint.scheme != "tcp" or endpoint.host != "127.0.0.1", do: raise("loopback TCP required")
{:ok, bridge} = :gen_tcp.connect(String.to_charlist(endpoint.host), endpoint.port, [:binary, active: false, packet: :line])
token = System.fetch_env!("WEBTEST_TOKEN")
send_frame.(bridge, ~s({"type":"hello","protocol_versions":[1],"sdk":"webtest-elixir-example","sdk_version":"0.1.0","token":"#{token}","capabilities":{"cancel":false,"events":false}}))
{:ok, hello} = :gen_tcp.recv(bridge, 0)
if not String.contains?(hello, ~s("hello_ok")), do: raise("hello rejected")

bridge_loop = fn loop ->
  case :gen_tcp.recv(bridge, 0) do
    {:ok, message} ->
      [_, type] = Regex.run(~r/"type"\s*:\s*"([^"]+)"/, message)
      [_, id] = Regex.run(~r/"id"\s*:\s*(\d+)/, message)
      case type do
        "describe" -> send_frame.(bridge, ~s({"type":"schema","id":#{id},"protocol":1,"schema_hash":"#{schema_hash}","functions":#{functions}}))
        "call" ->
          [_, email] = Regex.run(~r/"email"\s*:\s*"([^"]+)"/, message)
          admin = Regex.match?(~r/"admin"\s*:\s*true/, message)
          Agent.get_and_update(users, fn state ->
            if Map.has_key?(state, email) do
              {send_frame.(bridge, ~s({"type":"error","id":#{id},"code":"user.email_taken","message":"email already exists","retryable":false,"data":{}})), state}
            else
              user_id = map_size(state) + 1
              json = ~s({"type":"result","id":#{id},"value":{"id":#{user_id},"email":"#{email}","admin":#{admin}}})
              {send_frame.(bridge, json), Map.put(state, email, %{id: user_id, email: email, admin: admin})}
            end
          end)
        "ping" -> send_frame.(bridge, ~s({"type":"pong","id":#{id}}))
        "shutdown" -> send_frame.(bridge, ~s({"type":"shutdown_ok","id":#{id}})); :stop
      end
      if type != "shutdown", do: loop.(loop)
    _ -> :stop
  end
end
bridge_loop.(bridge_loop)
:gen_tcp.close(bridge)
:gen_tcp.close(listener)
