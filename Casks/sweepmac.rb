cask "sweepmac" do
  version "0.3.0"
  sha256 "18b4f6cfe953a88d5556ca64c47a48d6ffe494cff3391607e7f260b1ddc6e1e6"

  url "https://github.com/karol-blaszczyk/sweepmac/releases/download/v#{version}/sweepmac.dmg",
      verified: "github.com/karol-blaszczyk/sweepmac/"
  name "sweepmac"
  desc "Menu-bar app that finds and clears regenerable macOS caches"
  homepage "https://github.com/karol-blaszczyk/sweepmac"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :big_sur"

  app "sweepmac.app"

  # The CLI and GUI ship inside the bundle; expose them on PATH so the cask
  # covers everything the formula does.
  binary "#{appdir}/sweepmac.app/Contents/MacOS/sweepmac"
  binary "#{appdir}/sweepmac.app/Contents/MacOS/sweepmac-gui"

  zap trash: [
    "~/Library/Preferences/ai.sekondbrain.sweepmac.plist",
    "~/Library/Saved Application State/ai.sekondbrain.sweepmac.savedState",
  ]
end
