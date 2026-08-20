class Sweepmac < Formula
  desc "Find and clear regenerable macOS caches (CLI, GUI, and menu-bar app)"
  homepage "https://github.com/karol-blaszczyk/sweepmac"
  url "https://github.com/karol-blaszczyk/sweepmac.git",
      using: :git,
      tag:   "v0.1.0"
  version "0.1.0"
  license "MIT"
  head "https://github.com/karol-blaszczyk/sweepmac.git", using: :git, branch: "main"

  depends_on "rust" => :build
  depends_on :macos

  def install
    system "cargo", "install", "--locked", "--features", "gui tray",
           "--root", prefix, "--path", "."
  end

  def caveats
    <<~EOS
      Three binaries were installed:
        sweepmac        CLI  — scan is the default, nothing is deleted without --clean
        sweepmac-gui    native window with per-category checkboxes
        sweepmac-tray   🧹 menu-bar app (run from a terminal it also shows a Dock icon;
                        see the README for wrapping it in a .app to hide that)
    EOS
  end

  test do
    assert_match "sweepmac", shell_output("#{bin}/sweepmac --help")
  end
end
