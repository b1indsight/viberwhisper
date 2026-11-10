#!/usr/bin/env ruby
# Exercise the workflow's actual mode-selection and macOS command blocks without packaging.
require "fileutils"
require "open3"
require "shellwords"
require "tmpdir"
require "yaml"

def check(condition, message)
  abort message unless condition
end

root = File.expand_path("..", __dir__)
workflow = YAML.load_file(File.join(root, ".github/workflows/release.yml"))
# Psych's YAML 1.1 parser treats the unquoted GitHub Actions key `on` as true.
trigger = workflow.fetch("on") { workflow.fetch(true) }
inputs = trigger.fetch("workflow_dispatch").fetch("inputs")
check(inputs.fetch("diagnostic_logs").values_at("type", "default") == ["boolean", false],
      "Diagnostic logging must be an opt-in boolean.")

jobs = workflow.fetch("jobs")
preflight = jobs.fetch("preflight")
mode = preflight.fetch("steps").find { |step| step["id"] == "package-mode" }
check(mode, "Preflight must select and validate the package mode before packaging.")
check(mode.fetch("env") == {
        "PUBLISH_REQUESTED" => "${{ inputs.publish }}",
        "DIAGNOSTIC_LOGS_REQUESTED" => "${{ inputs.diagnostic_logs }}"
      }, "Package mode must use the dispatch booleans.")
%w[asset_suffix macos_features windows_features].each do |key|
  check(preflight.fetch("outputs").fetch(key) == "${{ steps.package-mode.outputs.#{key} }}",
        "Preflight must export #{key} from the validated mode.")
end

Dir.mktmpdir("release-package-contract-") do |fixture|
  [false, true].product([false, true]).each do |publish, diagnostic|
    output = File.join(fixture, "outputs")
    File.write(output, "")
    stdout, stderr, status = Open3.capture3(
      { "PUBLISH_REQUESTED" => publish.to_s, "DIAGNOSTIC_LOGS_REQUESTED" => diagnostic.to_s,
        "GITHUB_OUTPUT" => output }, "bash", "-euo", "pipefail", "-c", mode.fetch("run")
    )
    if publish && diagnostic
      check(!status.success? && stderr.include?("Diagnostic packages cannot be published"),
            "publish=true, diagnostic_logs=true must fail preflight: #{stdout}#{stderr}")
      check(File.zero?(output), "Rejected modes must not export package settings.")
      next
    end
    check(status.success?, "Allowed package mode failed: #{stdout}#{stderr}")
    selected = File.readlines(output, chomp: true).map { |line| line.split("=", 2) }.to_h
    expected_features = diagnostic ? "diagnostic-logs" : ""
    check(selected == {
            "asset_suffix" => diagnostic ? "-diagnostic" : "",
            "macos_features" => expected_features,
            "windows_features" => diagnostic ? "windows-app,diagnostic-logs" : "windows-app"
          }, "Unexpected package settings: #{selected}")

    { "package-macos" => "macos-universal", "package-windows" => "windows-x86_64" }.each do |job_id, platform|
      job = jobs.fetch(job_id)
      check(job.fetch("needs") == "preflight", "#{job_id} must wait for preflight.")
      features_key = job_id == "package-macos" ? "macos_features" : "windows_features"
      check(job.fetch("env").fetch("PACKAGE_FEATURES") == "${{ needs.preflight.outputs.#{features_key} }}",
            "#{job_id} must reuse the validated feature set.")
      check(job.fetch("env").fetch("ASSET_PREFIX") ==
              "ViberWhisper-${{ needs.preflight.outputs.tag }}-#{platform}${{ needs.preflight.outputs.asset_suffix }}",
            "#{job_id} distribution names must carry the mode suffix without changing ordinary names.")
      upload = job.fetch("steps").find { |step| step.fetch("uses", "").start_with?("actions/upload-artifact@") }
      artifact = job_id == "package-macos" ? "release-macos" : "release-windows"
      check(upload.fetch("with").fetch("name") == "#{artifact}${{ needs.preflight.outputs.asset_suffix }}",
            "#{job_id} workflow artifact must carry the same mode suffix.")
    end

    # A later cargo test can replace a release executable. Check every build/test/bundle call,
    # including cargo-bundle metadata even though the bundle deliberately skips rebuilding.
    macos = jobs.fetch("package-macos").fetch("steps").map do |step|
      step["run"] if step.fetch("run", "").match?(/\bcargo (build|test|bundle)\b/)
    end.compact
    %w[aarch64-apple-darwin x86_64-apple-darwin].each do |target|
      path = File.join(fixture, "target", target, "release")
      FileUtils.mkdir_p(path)
      File.write(File.join(path, "viberwhisper"), target)
    end
    commands = <<~'BASH'
      cargo() { printf 'cargo '; printf '%q ' "$@"; printf 'skip=%s\n' "${CARGO_BUNDLE_SKIP_BUILD:-0}"; }
      lipo() { :; }
    BASH
    stdout, stderr, status = Open3.capture3(
      { "PACKAGE_FEATURES" => expected_features }, "bash", "-euo", "pipefail", "-c",
      commands + macos.join("\n"), chdir: fixture
    )
    check(status.success?, "macOS command fixture failed: #{stderr}")
    calls = stdout.lines.map { |line| Shellwords.split(line) }
    check(calls.map { |args| args[1] } == %w[build build test bundle],
          "Expected both macOS builds, the release smoke test, and bundle: #{calls}")
    check(calls.take(2).map { |args| args[args.index("--target") + 1] }.sort ==
            %w[aarch64-apple-darwin x86_64-apple-darwin], "Both macOS architectures must be built.")
    calls.each do |args|
      index = args.index("--features")
      check(index && args[index + 1] == expected_features && args.include?("--release"),
            "macOS command lost release mode or selected features: #{args}")
      check(args.include?("--locked"), "macOS builds/tests must stay locked.") unless args[1] == "bundle"
    end
    check(calls.last.last == "skip=1", "cargo-bundle must preserve the already-built executable.")
    check(File.read(File.join(fixture, "target/release/viberwhisper")) == "aarch64-apple-darwin",
          "cargo-bundle must receive the previously built arm64 executable.")
  end
end

# PowerShell is not required by the local contract check. Inspect every Windows cargo call and
# delegated script so a newly introduced smoke build cannot silently replace either executable.
windows = jobs.fetch("package-windows").fetch("steps").map { |step| step.fetch("run", "") }.join("\n")
calls = windows.lines.grep(/\bcargo (build|test|run|bundle|rustc)\b/)
check(calls.length == 3, "Review the Windows build and smoke cargo invocations: #{calls}")
calls.each do |line|
  check(line.include?("--release --locked --target x86_64-pc-windows-msvc --features $env:PACKAGE_FEATURES"),
        "Windows build/test must preserve the CLI+GUI selected features: #{line}")
end
windows.scan(%r{\./(scripts/[\w-]+\.ps1)}).flatten.uniq.each do |script|
  check(!File.read(File.join(root, script), encoding: "UTF-8").match?(/\bcargo(?:\.exe)?\s+(build|test|run|bundle|rustc)\b/),
        "#{script} now invokes cargo; propagate and test the selected package features there too.")
end
check(jobs.fetch("publish").fetch("if").include?("!inputs.diagnostic_logs"),
      "Publication must also explicitly exclude diagnostic packages.")

puts "Release package mode fixtures passed (four input combinations; macOS executable commands; Windows command contract)."
