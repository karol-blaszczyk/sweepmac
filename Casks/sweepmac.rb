cask "sweepmac" do
  version "0.3.1"
  sha256 "8b6ad2eb5f10e5ea09bcfc7b19bcc3625d5da8e2d05c0aa72c701e7d59ac49b6"

  url "https://github.com/karol-blaszczyk/sweepmac/releases/download/v#{version}/sweepmac.dmg",
      verified: "github.com/karol-blaszczyk/sweepmac/"
  name "sweepmac"
  desc "Menu-bar app that finds and clears regenerable caches"
  homepage "https://github.com/karol-blaszczyk/sweepmac"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: :big_sur

  # The CLI and GUI ship inside the bundle, so linking them out of it makes the
  # cask cover everything the formula does.
  app "sweepmac.app"
  binary "#{appdir}/sweepmac.app/Contents/MacOS/sweepmac"
  binary "#{appdir}/sweepmac.app/Contents/MacOS/sweepmac-gui"

  zap trash: [
    "~/Library/Preferences/dev.sweepmac.sweepmac.plist",
    "~/Library/Saved Application State/dev.sweepmac.sweepmac.savedState",
    # Pre-0.3.2 installs used this bundle id; harmless to also trash it.
    "~/Library/Preferences/ai.sekondbrain.sweepmac.plist",
    "~/Library/Saved Application State/ai.sekondbrain.sweepmac.savedState",
  ]
end
