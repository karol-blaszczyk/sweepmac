cask "sweepmac" do
  version "0.3.3"
  sha256 "865c39329c87d91e33e86c79b051142dafaa6369599b394cc6b897a0316565ea"

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

  # The ai.sekondbrain.sweepmac entries are the pre-0.3.2 bundle id; harmless to
  # also trash them.
  zap trash: [
    "~/Library/Preferences/ai.sekondbrain.sweepmac.plist",
    "~/Library/Preferences/dev.sweepmac.sweepmac.plist",
    "~/Library/Saved Application State/ai.sekondbrain.sweepmac.savedState",
    "~/Library/Saved Application State/dev.sweepmac.sweepmac.savedState",
  ]
end
