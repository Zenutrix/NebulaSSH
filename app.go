package main

import (
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/pkg/sftp"
	"github.com/wailsapp/wails/v2/pkg/runtime"
	"github.com/zalando/go-keyring"
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
	SFTPClient *sftp.Client       // NEU: SFTP Client für den Datei-Manager
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

	// Wenn die Verbindung gerade noch aufgebaut wird -> SOFORT ABBRECHEN
	if sess.Cancel != nil {
		sess.Cancel()
	}

	if sess.SSHSession != nil {
		sess.SSHSession.Close()
	}
	if sess.SFTPClient != nil {
		sess.SFTPClient.Close()
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
func (a *App) Connect(id, ip, user, pass, privateKeyContent string) string {
	// 1. Context für den sofortigen Abbruch erstellen
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel() // Räumt den Context am Ende der Funktion auf

	a.mu.Lock()
	sess := &Session{
		ID:     id,
		Type:   "ssh",
		Cancel: cancel,
	}
	a.sessions[id] = sess
	a.mu.Unlock()

	// 2. Netzwerk-Verbindung MIT Abbruch-Möglichkeit und Timeout aufbauen
	dialer := net.Dialer{
		Timeout: 10 * time.Second, // OPTIMIERUNG: 10 Sekunden Timeout statt ewigem Hängen
	}
	netConn, err := dialer.DialContext(ctx, "tcp", ip+":22")
	if err != nil {
		a.Disconnect(id)
		return "Fehler beim Verbinden: " + err.Error()
	}

	// 3. SSH Konfiguration anwenden (Passwort oder Public Key)
	var authMethods []ssh.AuthMethod

	if privateKeyContent != "" {
		signer, err := ssh.ParsePrivateKey([]byte(privateKeyContent))
		if err == nil {
			authMethods = append(authMethods, ssh.PublicKeys(signer))
		} else {
			a.Disconnect(id)
			return "Ungültiger SSH-Key: " + err.Error()
		}
	}
	
	if pass != "" {
		authMethods = append(authMethods, ssh.Password(pass))
	}

	config := &ssh.ClientConfig{
		User:            user,
		Auth:            authMethods,
		HostKeyCallback: ssh.InsecureIgnoreHostKey(),
		Timeout:         10 * time.Second,
	}

	// 4. SSH Handshake über die abgebrochen-bare Netzwerkverbindung machen
	sshConn, chans, reqs, err := ssh.NewClientConn(netConn, ip+":22", config)
	if err != nil {
		netConn.Close()
		a.Disconnect(id)
		return "Handshake Fehler: " + err.Error()
	}
	client := ssh.NewClient(sshConn, chans, reqs)

	// SFTP Client direkt auf der bestehenden SSH Verbindung aufbauen
	var sftpClient *sftp.Client
	sftpClient, _ = sftp.NewClient(client)

	a.mu.Lock()
	if _, exists := a.sessions[id]; !exists {
		if sftpClient != nil {
			sftpClient.Close()
		}
		client.Close()
		a.mu.Unlock()
		return "Verbindung abgebrochen"
	}
	sess.SSHClient = client
	sess.SFTPClient = sftpClient
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
		// OPTIMIERUNG: Größerer Puffer für flüssigeres Scrolling bei viel Output
		buf := make([]byte, 8192)
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
	if _, exists := a.sessions[id]; !exists {
		port.Close()
		a.mu.Unlock()
		return "Abgebrochen"
	}
	sess.SerialPort = port
	a.mu.Unlock()

	go func() {
		// OPTIMIERUNG: Puffer auch für Serial auf 8 KB erhöht
		buf := make([]byte, 8192)
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

// --- VERSCHLÜSSELUNG FÜR HOSTS.JSON & KEYS.JSON ---

const keyringService = "NebulaSSH"
const keyringUser = "encryption_key"

func getEncryptionKey() ([]byte, error) {
	secret, err := keyring.Get(keyringService, keyringUser)
	if err != nil {
		key := make([]byte, 32)
		if _, err := io.ReadFull(rand.Reader, key); err != nil {
			return nil, err
		}
		secret = base64.StdEncoding.EncodeToString(key)
		err = keyring.Set(keyringService, keyringUser, secret)
		if err != nil {
			return nil, err
		}
		return key, nil
	}
	return base64.StdEncoding.DecodeString(secret)
}

func encryptData(plaintext []byte, key []byte) (string, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return "", err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", err
	}
	nonce := make([]byte, gcm.NonceSize())
	if _, err := io.ReadFull(rand.Reader, nonce); err != nil {
		return "", err
	}
	ciphertext := gcm.Seal(nonce, nonce, plaintext, nil)
	return base64.StdEncoding.EncodeToString(ciphertext), nil
}

func decryptData(cryptoText string, key []byte) ([]byte, error) {
	data, err := base64.StdEncoding.DecodeString(cryptoText)
	if err != nil {
		return nil, err
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return nil, err
	}
	nonceSize := gcm.NonceSize()
	if len(data) < nonceSize {
		return nil, errors.New("ciphertext too short")
	}
	nonce, ciphertext := data[:nonceSize], data[nonceSize:]
	return gcm.Open(nil, nonce, ciphertext, nil)
}

func (a *App) LoadHosts() string {
	exePath, err := os.Executable()
	if err != nil { return "[]" }
	dir := filepath.Dir(exePath)
	configPath := filepath.Join(dir, "hosts.json")

	data, err := os.ReadFile(configPath)
	if err != nil { return "[]" }

	if len(data) > 0 && data[0] == '[' {
		return string(data)
	}

	key, err := getEncryptionKey()
	if err != nil { return "[]" }

	decrypted, err := decryptData(string(data), key)
	if err != nil { 
		return "[]" 
	}
	return string(decrypted)
}

func (a *App) SaveHosts(hostsJson string) error {
	exePath, err := os.Executable()
	if err != nil { return err }
	dir := filepath.Dir(exePath)
	configPath := filepath.Join(dir, "hosts.json")

	key, err := getEncryptionKey()
	if err != nil { return err }

	encryptedData, err := encryptData([]byte(hostsJson), key)
	if err != nil { return err }

	return os.WriteFile(configPath, []byte(encryptedData), 0644)
}

// --- NEU: SSH KEY MANAGER SPEICHERUNG ---

func (a *App) LoadSSHKeys() string {
	exePath, err := os.Executable()
	if err != nil { return "[]" }
	dir := filepath.Dir(exePath)
	configPath := filepath.Join(dir, "keys.json")

	data, err := os.ReadFile(configPath)
	if err != nil { return "[]" }

	if len(data) > 0 && data[0] == '[' {
		return string(data)
	}

	key, err := getEncryptionKey()
	if err != nil { return "[]" }

	decrypted, err := decryptData(string(data), key)
	if err != nil { 
		return "[]" 
	}
	return string(decrypted)
}

func (a *App) SaveSSHKeys(keysJson string) error {
	exePath, err := os.Executable()
	if err != nil { return err }
	dir := filepath.Dir(exePath)
	configPath := filepath.Join(dir, "keys.json")

	key, err := getEncryptionKey()
	if err != nil { return err }

	encryptedData, err := encryptData([]byte(keysJson), key)
	if err != nil { return err }

	return os.WriteFile(configPath, []byte(encryptedData), 0644)
}

// ImportSSHKeyFromFile öffnet einen Dialog, um einen Private Key von der Festplatte einzulesen
func (a *App) ImportSSHKeyFromFile() string {
	localPath, err := runtime.OpenFileDialog(a.ctx, runtime.OpenDialogOptions{
		Title: "SSH Private Key auswählen",
	})
	if err != nil || localPath == "" { return "" }

	content, err := os.ReadFile(localPath)
	if err != nil { return "FEHLER:" + err.Error() }
	
	return string(content)
}

// --- SFTP DATEI-MANAGER ---

type FileInfo struct {
	Name  string `json:"name"`
	IsDir bool   `json:"isDir"`
	Size  int64  `json:"size"`
}

func (a *App) ListDirectory(id, path string) string {
	a.mu.Lock()
	sess, exists := a.sessions[id]
	a.mu.Unlock()

	if !exists || sess.Type != "ssh" || sess.SFTPClient == nil {
		return "[]"
	}

	files, err := sess.SFTPClient.ReadDir(path)
	if err != nil {
		return "[]"
	}

	var result []FileInfo
	if path != "/" {
		result = append(result, FileInfo{Name: "..", IsDir: true, Size: 0})
	}

	for _, f := range files {
		result = append(result, FileInfo{
			Name:  f.Name(),
			IsDir: f.IsDir(),
			Size:  f.Size(),
		})
	}

	jsonBytes, _ := json.Marshal(result)
	return string(jsonBytes)
}

func (a *App) ReadFile(id, path string) string {
	a.mu.Lock()
	sess, exists := a.sessions[id]
	a.mu.Unlock()

	if !exists || sess.SFTPClient == nil {
		return ""
	}

	file, err := sess.SFTPClient.Open(path)
	if err != nil {
		return "FEHLER:" + err.Error()
	}
	defer file.Close()

	content, err := io.ReadAll(file)
	if err != nil {
		return "FEHLER:" + err.Error()
	}
	return string(content)
}

func (a *App) WriteFile(id, path, content string) string {
	a.mu.Lock()
	sess, exists := a.sessions[id]
	a.mu.Unlock()

	if !exists || sess.SFTPClient == nil {
		return "FEHLER:Keine aktive SFTP Sitzung"
	}

	file, err := sess.SFTPClient.Create(path)
	if err != nil {
		return "FEHLER:" + err.Error()
	}
	defer file.Close()

	_, err = file.Write([]byte(content))
	if err != nil {
		return "FEHLER:" + err.Error()
	}
	return "OK"
}

// --- ERWEITERTES SFTP MANAGEMENT ---

func (a *App) DeleteFile(id, path string) string {
	a.mu.Lock()
	sess, exists := a.sessions[id]
	a.mu.Unlock()
	if !exists || sess.SFTPClient == nil { return "FEHLER: Keine aktive SFTP Sitzung" }

	stat, err := sess.SFTPClient.Stat(path)
	if err != nil { return "FEHLER:" + err.Error() }

	if stat.IsDir() {
		err = sess.SFTPClient.RemoveDirectory(path)
	} else {
		err = sess.SFTPClient.Remove(path)
	}

	if err != nil { return "FEHLER:" + err.Error() }
	return "OK"
}

func (a *App) RenameFile(id, oldPath, newPath string) string {
	a.mu.Lock()
	sess, exists := a.sessions[id]
	a.mu.Unlock()
	if !exists || sess.SFTPClient == nil { return "FEHLER: Keine aktive SFTP Sitzung" }

	if err := sess.SFTPClient.Rename(oldPath, newPath); err != nil {
		return "FEHLER:" + err.Error()
	}
	return "OK"
}

func (a *App) MakeDirectory(id, path string) string {
	a.mu.Lock()
	sess, exists := a.sessions[id]
	a.mu.Unlock()
	if !exists || sess.SFTPClient == nil { return "FEHLER: Keine aktive SFTP Sitzung" }

	if err := sess.SFTPClient.Mkdir(path); err != nil {
		return "FEHLER:" + err.Error()
	}
	return "OK"
}

// UploadFile öffnet einen Dialog und lädt eine Datei auf den Server hoch
func (a *App) UploadFile(id, remoteDir string) string {
	a.mu.Lock()
	sess, exists := a.sessions[id]
	a.mu.Unlock()
	if !exists || sess.SFTPClient == nil { return "FEHLER: Keine SFTP Sitzung" }

	localPath, err := runtime.OpenFileDialog(a.ctx, runtime.OpenDialogOptions{
		Title: "Datei für Upload auswählen",
	})
	if err != nil || localPath == "" { return "Abgebrochen" }

	localFile, err := os.Open(localPath)
	if err != nil { return "FEHLER:" + err.Error() }
	defer localFile.Close()

	// Baue Remote-Pfad sauber auf
	remotePath := strings.TrimSuffix(remoteDir, "/") + "/" + filepath.Base(localPath)
	remoteFile, err := sess.SFTPClient.Create(remotePath)
	if err != nil { return "FEHLER:" + err.Error() }
	defer remoteFile.Close()

	_, err = io.Copy(remoteFile, localFile)
	if err != nil { return "FEHLER:" + err.Error() }
	
	return "OK"
}

// DownloadFile lädt eine Datei vom Server herunter und öffnet einen Speicher-Dialog
func (a *App) DownloadFile(id, remotePath string) string {
	a.mu.Lock()
	sess, exists := a.sessions[id]
	a.mu.Unlock()
	if !exists || sess.SFTPClient == nil { return "FEHLER: Keine SFTP Sitzung" }

	filename := filepath.Base(remotePath)
	localPath, err := runtime.SaveFileDialog(a.ctx, runtime.SaveDialogOptions{
		Title:           "Speicherort auswählen",
		DefaultFilename: filename,
	})
	if err != nil || localPath == "" { return "Abgebrochen" }

	remoteFile, err := sess.SFTPClient.Open(remotePath)
	if err != nil { return "FEHLER:" + err.Error() }
	defer remoteFile.Close()

	localFile, err := os.Create(localPath)
	if err != nil { return "FEHLER:" + err.Error() }
	defer localFile.Close()

	_, err = io.Copy(localFile, remoteFile)
	if err != nil { return "FEHLER:" + err.Error() }

	return "OK"
}