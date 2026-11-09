package main

import (
	"embed"

	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
	"github.com/wailsapp/wails/v2/pkg/options/windows" // Wichtig für Windows-Effekte
)

//go:embed all:frontend/dist
var assets embed.FS

func main() {
	// Instanz der App erstellen
	app := NewApp()

	err := wails.Run(&options.App{
		Title:  "NebulaSSH",
		Width:  1200, // Etwas breiter für die Sidebar
		Height: 800,
		AssetServer: &assetserver.Options{
			Assets: assets,
		},
		// Hintergrundfarbe passend zum Svelte-Theme (dunkel)
		BackgroundColour: &options.RGBA{R: 15, G: 15, B: 23, A: 1},
		OnStartup:        app.startup,
		Bind: []interface{}{
			app,
		},
		// --- HIER DIE MAGIE FÜR DEN NATIVEN LOOK ---
		Windows: &windows.Options{
			WebviewIsTransparent: true,
			WindowIsTranslucent:  true,
			BackdropType:         windows.Acrylic, // Oder windows.Mica für Win11 Look
		},
	})

	if err != nil {
		println("Error:", err.Error())
	}
}
