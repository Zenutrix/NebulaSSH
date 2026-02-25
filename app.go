package main

import (
	"context"
	"io"
	"net"
	"os"
	"path/filepath"
	"sync"

	"github.com/wailsapp/wails/v2/pkg/runtime"
	"go.bug.st/serial"
	"golang.org/x/crypto/ssh"
)

// Session speichert die Daten für einen einzelnen Tab
type Session struct {
	ID         string
	Type       string
	SSHClient  *ssh.Client
	SSHSession *ssh.Session
	SSHStdin   io.WriteCloser
	SerialPort serial.Port
	Cancel     context.CancelFunc // NEU: Damit können wir Verbindungsversuche abbrechen!
}

type App struct {
	ctx      context.Context
	sessions map[string]*Session
	mu       sync.Mutex // Schützt die Map bei gleichzeitigen Zugriffen
}

func NewApp() *App {
	return &App{
		sessions: make(map[string]*Session),
	}
}

func (a *App) startup(ctx context.Context) {
	a.ctx = ctx
}

// Disconnect trennt eine spezifische Verbindung anhand der ID
func (a *App) Disconnect(id string) {
	a.mu.Lock()
	defer a.mu.Unlock()

	sess, exists := a.sessions[id]
	if !exists {
		return
	}

	// NEU: Wenn die Verbindung gerade noch aufgebaut wird -> SOFORT ABBRECHEN
	if sess.Cancel != nil {
		sess.Cancel()
	}

	if sess.SSHSession != nil {
		sess.SSHSession.Close()
	}
	if sess.SSHClient != nil {
		sess.SSHClient.Close()
	}
	if sess.SerialPort != nil {
		sess.SerialPort.Close()
	}

	delete(a.sessions, id)
}

// Connect baut eine SSH Verbindung für einen bestimmten Tab auf
func (a *App) Connect(id, ip, user, pass string) string {
	// 1. Context für den sofortigen Abbruch erstellen
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel() // Räumt den Context am Ende der Funktion auf

	a.mu.Lock()
	// Wir legen die Session schon VOR der Verbindung an, damit `Disconnect` sie abbrechen kann
	sess := &Session{
		ID:     id,
		Type:   "ssh",
		Cancel: cancel,
	}
	a.sessions[id] = sess
	a.mu.Unlock()

	// 2. Netzwerk-Verbindung MIT Abbruch-Möglichkeit aufbauen
	dialer := net.Dialer{}
	netConn, err := dialer.DialContext(ctx, "tcp", ip+":22")
	if err != nil {
		a.Disconnect(id) // Aufräumen
		return "Fehler beim Verbinden: " + err.Error()
	}

	// 3. SSH Konfiguration anwenden
	config := &ssh.ClientConfig{
		User:            user,
		Auth:            []ssh.AuthMethod{ssh.Password(pass)},
		HostKeyCallback: ssh.InsecureIgnoreHostKey(),
	}

	// 4. SSH Handshake über die abgebrochen-bare Netzwerkverbindung machen
	sshConn, chans, reqs, err := ssh.NewClientConn(netConn, ip+":22", config)
	if err != nil {
		netConn.Close()
		a.Disconnect(id)
		return "Handshake Fehler: " + err.Error()
	}
	client := ssh.NewClient(sshConn, chans, reqs)

	a.mu.Lock()
	// Prüfen, ob der Tab vielleicht in den letzten Millisekunden geschlossen wurde
	if _, exists := a.sessions[id]; !exists {
		client.Close()
		a.mu.Unlock()
		return "Verbindung abgebrochen"
	}
	sess.SSHClient = client
	a.mu.Unlock()

	// 5. Session aufbauen
	session, err := client.NewSession()
	if err != nil {
		a.Disconnect(id)
		return "Session Fehler: " + err.Error()
	}

	modes := ssh.TerminalModes{
		ssh.ECHO:          1,
		ssh.TTY_OP_ISPEED: 14400,
		ssh.TTY_OP_OSPEED: 14400,
	}
	if err := session.RequestPty("xterm", 40, 80, modes); err != nil {
		a.Disconnect(id)
		return "PTY Fehler: " + err.Error()
	}

	stdin, _ := session.StdinPipe()
	stdout, _ := session.StdoutPipe()

	a.mu.Lock()
	sess.SSHSession = session
	sess.SSHStdin = stdin
	a.mu.Unlock()

	go func() {
		buf := make([]byte, 1024)
		for {
			n, err := stdout.Read(buf)
			if n > 0 {
				runtime.EventsEmit(a.ctx, "server_data_"+id, string(buf[:n]))
			}
			if err != nil {
				a.Disconnect(id)
				runtime.EventsEmit(a.ctx, "server_closed_"+id, "\r\n[Verbindung getrennt]")
				break
			}
		}
	}()

	if err := session.Shell(); err != nil {
		a.Disconnect(id)
		return "Shell Fehler: " + err.Error()
	}

	return "Verbunden!"
}

// ConnectSerial baut eine serielle Verbindung für einen Tab auf
func (a *App) ConnectSerial(id, portName string, baudRate int) string {
	a.mu.Lock()
	sess := &Session{
		ID:   id,
		Type: "serial",
	}
	a.sessions[id] = sess
	a.mu.Unlock()

	mode := &serial.Mode{
		BaudRate: baudRate,
	}

	port, err := serial.Open(portName, mode)
	if err != nil {
		a.Disconnect(id)
		return "Serial Fehler: " + err.Error()
	}

	a.mu.Lock()
	// Prüfen, ob der Tab während des Öffnens geschlossen wurde
	if _, exists := a.sessions[id]; !exists {
		port.Close()
		a.mu.Unlock()
		return "Abgebrochen"
	}
	sess.SerialPort = port
	a.mu.Unlock()

	go func() {
		buf := make([]byte, 1024)
		for {
			a.mu.Lock()
			currentSess, exists := a.sessions[id]
			a.mu.Unlock()
			
			if !exists || currentSess.SerialPort == nil {
				break
			}
			
			n, err := currentSess.SerialPort.Read(buf)
			if n > 0 {
				runtime.EventsEmit(a.ctx, "server_data_"+id, string(buf[:n]))
			}
			if err != nil {
				a.Disconnect(id)
				runtime.EventsEmit(a.ctx, "server_closed_"+id, "\r\n[Port geschlossen]")
				break
			}
		}
	}()

	return "Verbunden!"
}

// GetSerialPorts liefert eine Liste aller verfügbaren COM-Ports
func (a *App) GetSerialPorts() []string {
	ports, err := serial.GetPortsList()
	if err != nil {
		return []string{}
	}
	return ports
}

// SendData schickt Tastatureingaben vom UI an den richtigen Server
func (a *App) SendData(id, data string) {
	a.mu.Lock()
	sess, exists := a.sessions[id]
	a.mu.Unlock()

	if !exists {
		return
	}

	if sess.Type == "ssh" && sess.SSHStdin != nil {
		sess.SSHStdin.Write([]byte(data))
	} else if sess.Type == "serial" && sess.SerialPort != nil {
		sess.SerialPort.Write([]byte(data))
	}
}

// LoadHosts liest die hosts.json aus dem Programmverzeichnis
func (a *App) LoadHosts() string {
	exePath, err := os.Executable()
	if err != nil { return "[]" }
	dir := filepath.Dir(exePath)
	configPath := filepath.Join(dir, "hosts.json")

	data, err := os.ReadFile(configPath)
	if err != nil { return "[]" }
	return string(data)
}

// SaveHosts speichert die hosts.json im Programmverzeichnis
func (a *App) SaveHosts(hostsJson string) error {
	exePath, err := os.Executable()
	if err != nil { return err }
	dir := filepath.Dir(exePath)
	configPath := filepath.Join(dir, "hosts.json")

	return os.WriteFile(configPath, []byte(hostsJson), 0644)
}