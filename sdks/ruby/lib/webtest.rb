require "json"
require "socket"
require "thread"
require "uri"

module WebTest
  SDK_INFO = {
    name: "webtest-ruby", version: "0.1.0", minimum_protocol: 1, maximum_protocol: 1,
    generated_schema_revision: 1, transports: %w[unix tcp stdio]
  }.freeze

  class ApplicationError < StandardError
    attr_reader :code, :retryable, :data

    def initialize(message, code: "application.error", retryable: false, data: {})
      super(message)
      @code = code
      @retryable = retryable
      @data = data
    end
  end

  class Cancelled < ApplicationError
    def initialize
      super("call was cancelled", code: "call.cancelled")
    end
  end

  class AppBridge
    DEFAULT_MAX = 1_048_576

    def initialize(manifest, max_message_bytes: DEFAULT_MAX, max_pending_calls: 1_024,
                   max_events_per_call: 1_024, logger: nil)
      validate_manifest(manifest)
      @manifest = Marshal.load(Marshal.dump(manifest))
      @max = max_message_bytes
      @max_pending_calls = max_pending_calls
      @max_events_per_call = max_events_per_call
      @logger = logger || proc { |message| $stderr.puts("[webtest] #{message}") }
      @handlers = {}
      @in_flight = {}
      @mutex = Mutex.new
      @write_mutex = Mutex.new
    end

    def register(name, &handler)
      raise ArgumentError, "function #{name} is not in the manifest" unless @manifest.fetch("functions").key?(name)
      raise ArgumentError, "function #{name} is already registered" if @handlers.key?(name)
      raise ArgumentError, "handler block is required" unless handler
      @handlers[name] = handler
      self
    end

    def export_schema(path)
      directory = File.dirname(path)
      Dir.mkdir(directory, 0o700) unless directory == "." || Dir.exist?(directory)
      File.open(path, "w", 0o600) do |file|
        file.write(JSON.pretty_generate(sort_json(@manifest)) + "\n")
      end
    end

    def connect_from_env
      token = ENV["WEBTEST_TOKEN"]
      raise "WEBTEST_TOKEN is required; bridge helpers must run only under WebTest" if token.nil? || token.empty?
      reader, writer, closer = connect(ENV.fetch("WEBTEST_BRIDGE", "stdio"))
      send_frame(writer, {
        "type" => "hello", "protocol_versions" => [1], "sdk" => SDK_INFO[:name],
        "sdk_version" => SDK_INFO[:version], "token" => token,
        "capabilities" => { "cancel" => true, "events" => true }
      })
      hello = read_frame(reader)
      raise "bridge rejected hello: #{hello}" if hello && hello["type"] == "hello_error"
      raise "expected protocol-1 hello_ok" unless hello && hello["type"] == "hello_ok" && hello["protocol"] == 1
      @max = [@max, hello.fetch("max_message_bytes")].min
      while (message = read_frame(reader))
        case message["type"]
        when "describe"
          send_frame(writer, "type" => "schema", "id" => message["id"], "protocol" => 1,
                     "schema_hash" => @manifest["schema_hash"], "functions" => @manifest["functions"])
        when "call"
          dispatch_call(message, writer)
        when "cancel"
          thread = @mutex.synchronize { @in_flight[message["id"]] }
          thread.raise(Cancelled.new) if thread
        when "ping"
          send_frame(writer, "type" => "pong", "id" => message["id"])
        when "shutdown"
          threads = @mutex.synchronize { @in_flight.values.dup }
          threads.each { |thread| thread.raise(Cancelled.new) if thread.alive? }
          threads.each { |thread| thread.join(1) }
          send_frame(writer, "type" => "shutdown_ok", "id" => message["id"])
          closer.call
          return
        when "pong"
          next
        else
          raise "unknown message type #{message["type"]}"
        end
      end
      raise "bridge EOF with calls pending" unless @mutex.synchronize { @in_flight.empty? }
    ensure
      closer.call if defined?(closer) && closer
    end

    private

    def sort_json(value)
      case value
      when Hash
        value.keys.sort.each_with_object({}) { |key, sorted| sorted[key] = sort_json(value[key]) }
      when Array
        value.map { |item| sort_json(item) }
      else
        value
      end
    end

    def dispatch_call(message, writer)
      id = message.fetch("id")
      raise "invalid request ID" unless id.is_a?(Integer) && id >= 0
      deadline_ms = message.fetch("deadline_ms")
      raise "invalid call deadline" unless deadline_ms.is_a?(Integer) && deadline_ms.positive?
      function = message.fetch("function")
      schema = @manifest.fetch("functions")[function]
      handler = @handlers[function]
      unless schema && handler
        send_frame(writer, "type" => "error", "id" => id, "code" => "function.unknown",
                   "message" => "unknown function", "retryable" => false, "data" => {})
        return
      end
      gate = Queue.new
      thread = Thread.new do
        deadline = nil
        begin
          gate.pop
          deadline = Thread.new do
            sleep(deadline_ms / 1000.0)
            thread.raise(ApplicationError.new("call deadline elapsed", code: "call.deadline"))
          end
          arguments = with_defaults(schema.fetch("params"), message.fetch("arguments"))
          validate_value(schema.fetch("params"), arguments, "$.arguments")
          event_count = 0
          emit = proc do |kind, value|
            event_count += 1
            raise "too many call events" if event_count > @max_events_per_call
            validate_transferable(value, "$.event.value")
            send_frame(writer, "type" => "event", "call_id" => id,
                       "kind" => bounded_string(kind.to_s, 128), "value" => value)
          end
          value = handler.call(arguments, emit)
          validate_value(schema.fetch("returns"), value, "$.result")
          send_frame(writer, "type" => "result", "id" => id, "value" => value)
        rescue ApplicationError => error
          data = transferable?(error.data) ? error.data : {}
          send_frame(writer, "type" => "error", "id" => id, "code" => error.code,
                     "message" => bounded_string(error.message, 4096),
                     "retryable" => error.retryable, "data" => data)
        rescue StandardError => error
          send_frame(writer, "type" => "error", "id" => id, "code" => "application.error",
                     "message" => bounded_string(error.message, 4096),
                     "retryable" => false, "data" => {})
        ensure
          deadline&.kill
          @mutex.synchronize { @in_flight.delete(id) }
        end
      end
      @mutex.synchronize do
        raise "duplicate request ID #{id}" if @in_flight.key?(id)
        raise "too many pending calls" if @in_flight.length >= @max_pending_calls
        @in_flight[id] = thread
      end
      gate << true
    end

    def with_defaults(schema, value)
      result = value.dup
      return result unless schema["type"] == "object"
      schema.fetch("fields").each do |name, field|
        result[name] = Marshal.load(Marshal.dump(field["default"])) if !result.key?(name) && field.key?("default")
      end
      result
    end

    def validate_value(schema, value, path, depth = 0)
      raise "#{path} exceeds value depth" if depth > 32
      return validate_value(schema.fetch("base"), value, path, depth + 1) if schema["type"] == "alias"
      return if schema["type"] == "optional" && value.nil?
      return validate_value(schema.fetch("item"), value, path, depth + 1) if schema["type"] == "optional"
      valid = case schema["type"]
              when "null" then value.nil?
              when "boolean" then value == true || value == false
              when "integer" then value.is_a?(Integer)
              when "float" then value.is_a?(Numeric) && value.finite?
              when "string" then value.is_a?(String) && value.bytesize <= DEFAULT_MAX
              when "array" then value.is_a?(Array)
              when "object" then value.is_a?(Hash)
              else false
              end
      raise "#{path} does not match #{schema["type"]}" unless valid
      if schema["type"] == "array"
        raise "#{path} has too many items" if value.length > 10_000
        value.each_with_index do |item, index|
          validate_value(schema.fetch("items"), item, "#{path}[#{index}]", depth + 1)
        end
      end
      if schema["type"] == "object"
        raise "#{path} has too many fields" if value.length > 10_000
        value.each_key { |key| raise "#{path}.#{key} is not declared" unless schema.fetch("fields").key?(key) }
        schema.fetch("fields").each do |name, field|
          raise "#{path}.#{name} is required" if !value.key?(name) && !field["optional"]
          validate_value(field, value[name], "#{path}.#{name}", depth + 1) if value.key?(name)
        end
      end
    end

    def validate_transferable(value, path, depth = 0)
      raise "#{path} exceeds value depth" if depth > 32
      case value
      when NilClass, TrueClass, FalseClass, Integer
        nil
      when Float
        raise "#{path} is not finite" unless value.finite?
      when String
        raise "#{path} string is too large" if value.bytesize > DEFAULT_MAX
      when Array
        raise "#{path} has too many items" if value.length > 10_000
        value.each_with_index { |item, index| validate_transferable(item, "#{path}[#{index}]", depth + 1) }
      when Hash
        raise "#{path} has too many fields" if value.length > 10_000
        value.each { |name, item| validate_transferable(item, "#{path}.#{name}", depth + 1) }
      else
        raise "#{path} is not transferable"
      end
    end

    def transferable?(value)
      validate_transferable(value, "$.error.data")
      true
    rescue StandardError
      false
    end

    def bounded_string(value, bytes)
      value.to_s.each_char.each_with_object("") do |character, result|
        break result if result.bytesize + character.bytesize > bytes
        result << character
      end
    end

    def read_frame(reader)
      line = reader.gets(@max + 2)
      return nil unless line
      raise "frame too large" if line.bytesize > @max + 1
      raise "truncated bridge frame" unless line.end_with?("\n")
      line = line.dup.force_encoding(Encoding::UTF_8)
      raise "invalid UTF-8 bridge frame" unless line.valid_encoding?
      value = JSON.parse(line)
      raise "message must be an object" unless value.is_a?(Hash)
      value
    end

    def send_frame(writer, message)
      encoded = JSON.generate(message)
      raise "frame too large" if encoded.bytesize > @max
      @write_mutex.synchronize do
        writer.write(encoded + "\n")
        writer.flush
      end
    end

    def connect(endpoint)
      return [$stdin, $stdout, proc {}] if endpoint == "stdio" || endpoint == "stdio:"
      socket = if endpoint.start_with?("unix:")
                 UNIXSocket.new(endpoint.delete_prefix("unix:"))
               elsif endpoint.start_with?("tcp://")
                 uri = URI(endpoint)
                 raise "refusing non-loopback bridge endpoint" unless ["127.0.0.1", "localhost", "::1"].include?(uri.host)
                 TCPSocket.new(uri.host, uri.port)
               else
                 raise "unsupported bridge endpoint #{endpoint}"
               end
      [socket, socket, proc { socket.close unless socket.closed? }]
    end

    def validate_manifest(manifest)
      valid = manifest["manifest_version"] == 1 && manifest["protocol"] == 1 && manifest["provider"] == "app"
      valid &&= manifest["schema_hash"].to_s.match?(/\Ablake3:[0-9a-f]{64}\z/)
      valid &&= manifest["sdk"].is_a?(String) && !manifest["sdk"].empty? && manifest["sdk"].bytesize <= 128
      valid &&= manifest["sdk_version"].is_a?(String) && !manifest["sdk_version"].empty? && manifest["sdk_version"].bytesize <= 64
      raise ArgumentError, "invalid protocol-1 app manifest" unless valid
      raise ArgumentError, "manifest has too many functions" if manifest.fetch("functions").length > 10_000
      manifest.fetch("functions").each do |name, schema|
        raise ArgumentError, "invalid function name #{name}" unless name.match?(/\A[A-Za-z_][A-Za-z0-9_]{0,127}\z/)
        validate_documentation(schema.fetch("documentation", ""), "documentation for #{name}")
        raise ArgumentError, "function #{name} parameters must be an object" unless schema.dig("params", "type") == "object"
        validate_schema_shape(schema.fetch("params"), "$.functions.#{name}.params")
        validate_schema_shape(schema.fetch("returns"), "$.functions.#{name}.returns")
        schema.fetch("params").fetch("fields").each do |field_name, field|
          next unless field.key?("default")
          raise ArgumentError, "#{name}.#{field_name} default requires optional" unless field["optional"]
          validate_value(field, field["default"], "$.functions.#{name}.params.#{field_name}.default")
        end
      end
    end

    def validate_schema_shape(schema, path, depth = 0)
      raise ArgumentError, "#{path} is not a bounded schema" if depth > 32 || !schema.is_a?(Hash)
      return if %w[null boolean integer float string].include?(schema["type"])
      return validate_schema_shape(schema.fetch("items"), "#{path}.items", depth + 1) if schema["type"] == "array"
      return validate_schema_shape(schema.fetch("item"), "#{path}.item", depth + 1) if schema["type"] == "optional"
      return validate_schema_shape(schema.fetch("base"), "#{path}.base", depth + 1) if schema["type"] == "alias"
      if schema["type"] == "object" && schema["fields"].is_a?(Hash) && schema["fields"].length <= 10_000
        schema.fetch("fields").each do |name, field|
          raise ArgumentError, "#{path} has invalid field #{name}" unless name.match?(/\A[A-Za-z_][A-Za-z0-9_]{0,127}\z/)
          validate_documentation(field.fetch("documentation", ""), "#{path}.#{name} documentation")
          validate_schema_shape(field, "#{path}.#{name}", depth + 1)
        end
        return
      end
      raise ArgumentError, "#{path} has unknown or invalid type #{schema["type"]}"
    end

    def validate_documentation(value, subject)
      unless value.is_a?(String) && value.bytesize <= 1_024
        raise ArgumentError, "#{subject} is not bounded text"
      end
      invalid = value.each_codepoint.any? do |codepoint|
        (codepoint < 32 && ![9, 10].include?(codepoint)) || codepoint == 127
      end
      raise ArgumentError, "#{subject} contains a control character" if invalid
    end
  end
end
