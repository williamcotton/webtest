require "json"
require "minitest/autorun"
require "tmpdir"
require_relative "lib/webtest"

class WebTestSdkTest < Minitest::Test
  def test_registration_is_schema_bound
    manifest = JSON.parse(File.read(File.expand_path("../../protocol/examples/app-schema.json", __dir__)))
    bridge = WebTest::AppBridge.new(manifest)
    bridge.register("create_user") { {} }
    assert_raises(ArgumentError) { bridge.register("create_user") { {} } }
    assert_raises(ArgumentError) { bridge.register("missing") { {} } }
  end

  def test_manifest_metadata_and_documentation_are_bounded
    manifest = JSON.parse(File.read(File.expand_path("../../protocol/examples/app-schema.json", __dir__)))
    assert_raises(ArgumentError) { WebTest::AppBridge.new(manifest.merge("sdk" => "")) }
    invalid = Marshal.load(Marshal.dump(manifest))
    invalid["functions"]["create_user"]["documentation"] = "invalid\0documentation"
    assert_raises(ArgumentError) { WebTest::AppBridge.new(invalid) }
  end

  def test_schema_export_is_deterministic_across_hash_insertion_order
    manifest = JSON.parse(File.read(File.expand_path("../../protocol/examples/app-schema.json", __dir__)))
    Dir.mktmpdir("webtest-ruby-sdk-") do |directory|
      first = File.join(directory, "first.json")
      second = File.join(directory, "second.json")
      WebTest::AppBridge.new(manifest).export_schema(first)
      WebTest::AppBridge.new(reverse_json(manifest)).export_schema(second)
      assert_equal File.binread(first), File.binread(second)
      assert_equal manifest, JSON.parse(File.read(first))
    end
  end

  private

  def reverse_json(value)
    case value
    when Hash
      value.to_a.reverse.to_h { |key, item| [key, reverse_json(item)] }
    when Array
      value.map { |item| reverse_json(item) }
    else
      value
    end
  end
end
