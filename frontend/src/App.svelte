<script>
  import { onMount, tick } from 'svelte';
  import { Terminal } from 'xterm';
  import { FitAddon } from 'xterm-addon-fit';
  import { SearchAddon } from '@xterm/addon-search'; 
  import { Connect, ConnectSerial, GetSerialPorts, Disconnect, SendData, SaveHosts, LoadHosts } from '../wailsjs/go/main/App';
  import { EventsOn, EventsOff } from '../wailsjs/runtime/runtime';
  
  import 'xterm/css/xterm.css';

  // --- Modus & Quick Connect ---
  let mode = 'ssh';
  let qcName = ''; 
  let qcIp = '';
  let qcUser = 'root'; 
  let qcPass = '';
  let serialPorts = [];
  let selectedPort = '';
  
  let selectedBaudLabel = '115200';
  
  let isConnecting = false;
  let savedHosts = [];

  // --- TAB SYSTEM ---
  let sessions = [];
  let activeSessionId = null;

  // --- SUCHFUNKTION (Strg + F) ---
  let showSearchBar = false;
  let searchTerm = '';

  // --- BAUDRATEN PROFILE ---
  let showBaudModal = false;
  let newBaudName = '';
  let newBaudRate = null; 
  const defaultProfiles = [
    { rate: 9600, label: '9600' },
    { rate: 19200, label: '19200' },
    { rate: 38400, label: '38400' },
    { rate: 57600, label: '57600' },
    { rate: 115200, label: '115200' }
  ];
  let baudProfiles = [...defaultProfiles];

  // --- MAKROS / SNIPPETS ---
  let savedSnippets = [];
  let showSnippetModal = false;
  let newSnippetName = '';
  let newSnippetCmd = '';

  onMount(async () => {
    window.addEventListener('resize', () => {
      const activeSession = sessions.find(s => s.id === activeSessionId);
      if (activeSession && activeSession.fitAddon) {
        activeSession.fitAddon.fit();
      }
    });

    // Globaler Keydown-Listener für Strg + F
    window.addEventListener('keydown', (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') {
        if (activeSessionId) {
          e.preventDefault(); 
          showSearchBar = true;
          setTimeout(() => document.getElementById('term-search-input')?.focus(), 50);
        }
      }
      
      if (e.key === 'Escape' && showSearchBar) {
        closeSearch();
      }
    });

    try {
      const data = await LoadHosts();
      if (data) savedHosts = JSON.parse(data);
    } catch (e) {}

    try {
      const savedBauds = localStorage.getItem('nebula_baud_profiles');
      if (savedBauds) {
        const parsedBauds = JSON.parse(savedBauds);
        const customProfiles = parsedBauds.filter(p => !defaultProfiles.some(dp => dp.label === p.label));
        baudProfiles = [...defaultProfiles, ...customProfiles].sort((a, b) => a.rate - b.rate);
      }
    } catch(e) {}

    try {
      const storedSnippets = localStorage.getItem('nebula_snippets');
      if (storedSnippets) savedSnippets = JSON.parse(storedSnippets);
      else {
        savedSnippets = [
          { name: 'Update', cmd: 'sudo apt update && sudo apt upgrade -y\n' },
          { name: 'Reboot', cmd: 'sudo reboot\n' }
        ];
      }
    } catch(e) {}
  });

  async function loadSerialPorts() {
    try {
      serialPorts = await GetSerialPorts();
      if (serialPorts.length > 0 && !selectedPort) {
        selectedPort = serialPorts[0];
      }
    } catch (e) {}
  }

  async function createTab(title, type, details) {
    const id = "sess_" + Date.now() + "_" + Math.floor(Math.random() * 1000);
    const newSession = { id, title, type, term: null, fitAddon: null, searchAddon: null, details };
    
    sessions = [...sessions, newSession];
    activeSessionId = id;

    await tick();

    const container = document.getElementById(`term-${id}`);
    const term = new Terminal({
      cursorBlink: true, fontSize: 14, fontFamily: '"Cascadia Code", "Courier New", monospace',
      scrollback: 50000,
      theme: { 
        background: '#0f0f17', 
        foreground: '#ffffff', 
        cursor: '#6200ee', 
        selectionBackground: '#6200ee80',
        selectionInactiveBackground: '#6200ee80' 
      }
    });
    
    term.attachCustomKeyEventHandler((e) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') {
        if (e.type === 'keydown') {
          showSearchBar = true;
          setTimeout(() => document.getElementById('term-search-input')?.focus(), 50);
        }
        return false; 
      }
      return true; 
    });
    
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);

    const searchAddon = new SearchAddon();
    term.loadAddon(searchAddon);

    term.open(container);
    fitAddon.fit();

    term.onData(data => SendData(id, data));
    EventsOn(`server_data_${id}`, data => term.write(data));
    EventsOn(`server_closed_${id}`, data => term.write(data));

    term.onSelectionChange(() => {
      if (term.hasSelection()) {
        // BUGFIX: Wenn die Suchleiste fokussiert ist (wir also tippen oder Buttons klicken),
        // brechen wir das Kopieren sofort ab! So wird kein Fokus gestohlen.
        const active = document.activeElement;
        if (active && (active.id === 'term-search-input' || active.closest('.search-bar'))) {
          return;
        }

        const selection = term.getSelection();
        const textArea = document.createElement("textarea");
        textArea.value = selection;
        document.body.appendChild(textArea);
        textArea.select(); 
        document.execCommand("copy");
        document.body.removeChild(textArea);
        
        // Wenn der Nutzer manuell markiert hat, Terminal im Fokus lassen
        term.focus();
      }
    });

    container.addEventListener('contextmenu', async (e) => {
      e.preventDefault();
      try {
        const text = await navigator.clipboard.readText();
        SendData(id, text);
      } catch (err) {}
    });

    newSession.term = term;
    newSession.fitAddon = fitAddon;
    newSession.searchAddon = searchAddon; 
    sessions = [...sessions]; 
    return id; 
  }

  function switchTab(id) {
    activeSessionId = id;
    
    const activeSession = sessions.find(s => s.id === id);
    if (activeSession && activeSession.details) {
      mode = activeSession.type;
      qcName = activeSession.details.name || '';
      
      if (mode === 'ssh') {
        qcIp = activeSession.details.ip || '';
        qcUser = activeSession.details.user || '';
        qcPass = activeSession.details.pass || '';
      } else if (mode === 'serial') {
        selectedPort = activeSession.details.port || '';
        selectedBaudLabel = activeSession.details.baudLabel || '115200';
      }
    }

    setTimeout(() => {
      if (activeSession && activeSession.fitAddon) activeSession.fitAddon.fit();
    }, 10);
  }

  async function closeTab(id, event) {
    if (event) event.stopPropagation();
    await Disconnect(id);
    EventsOff(`server_data_${id}`);
    EventsOff(`server_closed_${id}`);

    const sessionIndex = sessions.findIndex(s => s.id === id);
    if (sessionIndex > -1) {
      if (sessions[sessionIndex].term) sessions[sessionIndex].term.dispose();
      sessions.splice(sessionIndex, 1);
      sessions = [...sessions];
    }

    if (activeSessionId === id) {
      if (sessions.length > 0) {
        switchTab(sessions[Math.max(0, sessionIndex - 1)].id);
      } else {
        activeSessionId = null;
        qcName = ''; qcIp = ''; qcPass = '';
        showSearchBar = false;
      }
    }
  }

  async function handleConnect() {
    isConnecting = true;

    try {
      if (mode === 'ssh') {
        if (!qcIp || !qcUser || !qcPass) throw "Bitte IP, Benutzer und Passwort eingeben!";
        
        const tabTitle = qcName ? qcName : `${qcUser}@${qcIp}`;
        const sessId = await createTab(tabTitle, 'ssh', { name: qcName, ip: qcIp, user: qcUser, pass: qcPass });
        
        const term = sessions.find(s => s.id === sessId).term;
        term.writeln(`\x1b[1;32mBaue SSH Verbindung zu ${qcIp} auf...\x1b[0m`);
        
        let result = await Connect(sessId, qcIp, qcUser, qcPass);
        if (result !== "Verbunden!") term.writeln(`\x1b[1;31m[Fehler] ${result}\x1b[0m`);

      } else if (mode === 'serial') {
        if (!selectedPort) throw "Kein COM-Port ausgewählt!";
        
        const activeBaudRate = baudProfiles.find(p => p.label === selectedBaudLabel)?.rate || 115200;
        const tabTitle = qcName ? qcName : `${selectedPort} (${activeBaudRate})`;
        
        const sessId = await createTab(tabTitle, 'serial', { name: qcName, port: selectedPort, baudLabel: selectedBaudLabel });
        
        const term = sessions.find(s => s.id === sessId).term;
        term.writeln(`\x1b[1;32mÖffne Serial Port ${selectedPort} mit ${activeBaudRate} Baud...\x1b[0m`);
        
        let result = await ConnectSerial(sessId, selectedPort, activeBaudRate);
        if (result !== "Verbunden!") term.writeln(`\x1b[1;31m[Fehler] ${result}\x1b[0m`);
      }
    } catch (e) {
      alert(e); 
    }
    
    isConnecting = false;
  }

  function editDevice(host) {
    mode = host.type || 'ssh';
    qcName = host.name || '';
    if (mode === 'ssh') { 
      qcIp = host.ip || ''; qcUser = host.user || 'root'; qcPass = host.pass || ''; 
    } else { 
      selectedPort = host.port || ''; 
      const foundProfile = baudProfiles.find(p => p.rate === (host.baud || 115200));
      selectedBaudLabel = foundProfile ? foundProfile.label : '115200';
      loadSerialPorts(); 
    }
  }

  async function connectToSavedDevice(host) {
    editDevice(host);
    await handleConnect();
  }

  async function saveDevice() {
    if (mode !== 'ssh') return;
    const newHost = { type: mode, name: qcName || qcIp, ip: qcIp, user: qcUser, pass: qcPass };
    if (!qcIp || !qcUser) return alert("IP & Benutzer benötigt.");

    const existsIndex = savedHosts.findIndex(h => h.name === qcName || h.ip === qcIp);
    if (existsIndex >= 0) savedHosts[existsIndex] = newHost; else savedHosts.push(newHost);
    
    savedHosts = [...savedHosts];
    await SaveHosts(JSON.stringify(savedHosts, null, 2));
  }

  async function deleteDevice(host) {
    savedHosts = savedHosts.filter(h => h !== host);
    await SaveHosts(JSON.stringify(savedHosts, null, 2));
  }

  function copyPassword() {
    if (!qcPass) return;
    const textArea = document.createElement("textarea");
    textArea.value = qcPass;
    document.body.appendChild(textArea);
    textArea.select();
    document.execCommand("copy");
    document.body.removeChild(textArea);
  }

  // --- SUCH FUNKTIONEN ---
  function triggerSearch(incremental = false) {
    if (!activeSessionId) return;
    const session = sessions.find(s => s.id === activeSessionId);
    
    if (session && session.searchAddon) {
      if (!searchTerm) {
        if (session.term) session.term.clearSelection();
        return;
      }
      
      session.searchAddon.findNext(searchTerm, { incremental: incremental });
    }
  }

  function findPrevious() {
    if (!searchTerm || !activeSessionId) return;
    const session = sessions.find(s => s.id === activeSessionId);
    if (session && session.searchAddon) {
      session.searchAddon.findPrevious(searchTerm);
    }
  }

  function closeSearch() {
    showSearchBar = false;
    searchTerm = ''; 
    const session = sessions.find(s => s.id === activeSessionId);
    if (session && session.term) {
      session.term.clearSelection(); 
      session.term.focus(); 
    }
  }

  // --- BAUDRATEN FUNKTIONEN ---
  function addBaudProfile() {
    const rate = parseInt(newBaudRate);
    if (isNaN(rate) || rate <= 0) return alert("Bitte eine gültige Zahl für die Baudrate eingeben.");
    const label = newBaudName ? `${rate} (${newBaudName})` : `${rate} (Custom)`;
    baudProfiles = [...baudProfiles, { rate, label }];
    baudProfiles.sort((a, b) => a.rate - b.rate);
    selectedBaudLabel = label; 
    localStorage.setItem('nebula_baud_profiles', JSON.stringify(baudProfiles));
    showBaudModal = false;
  }

  function deleteBaudProfile() {
    if (defaultProfiles.some(p => p.label === selectedBaudLabel)) return; 
    baudProfiles = baudProfiles.filter(p => p.label !== selectedBaudLabel);
    localStorage.setItem('nebula_baud_profiles', JSON.stringify(baudProfiles));
    selectedBaudLabel = '115200'; 
  }

  // --- MAKRO FUNKTIONEN ---
  function executeSnippet(cmd) {
    if (activeSessionId) SendData(activeSessionId, cmd);
    else alert("Bitte öffne zuerst einen Terminal-Tab, um das Makro auszuführen.");
  }

  function addSnippet() {
    if (!newSnippetName || !newSnippetCmd) return alert("Bitte Name und Befehl eingeben!");
    let finalCmd = newSnippetCmd;
    if (!finalCmd.endsWith('\n')) finalCmd += '\n';
    savedSnippets = [...savedSnippets, { name: newSnippetName, cmd: finalCmd }];
    localStorage.setItem('nebula_snippets', JSON.stringify(savedSnippets));
    showSnippetModal = false;
  }

  function deleteSnippet(snip, event) {
    event.stopPropagation();
    savedSnippets = savedSnippets.filter(s => s !== snip);
    localStorage.setItem('nebula_snippets', JSON.stringify(savedSnippets));
  }
</script>

<main>
  <!-- Linke Sidebar -->
  <nav class="sidebar">
    <div class="logo-area">
      <span class="logo-text">NEBULA</span><span class="logo-dot">SSH</span>
    </div>
    
    <div class="sidebar-layout">
      <!-- OBERER TEIL: Quick Connect -->
      <div class="quick-connect-container" style={mode === 'serial' ? 'flex: 1;' : ''}>
        <div class="section-title">VERBINDUNG {mode === 'ssh' ? '/ NEUES GERÄT' : ''}</div>
        <div class="mode-toggle">
          <button class="toggle-btn {mode === 'ssh' ? 'active' : ''}" on:click={() => { mode = 'ssh'; }} disabled={isConnecting}>SSH</button>
          <button class="toggle-btn {mode === 'serial' ? 'active' : ''}" on:click={() => { mode = 'serial'; qcName = ''; loadSerialPorts(); }} disabled={isConnecting}>Serial</button>
        </div>
        
        {#if mode === 'ssh'}
          <div class="form-group">
            <label for="input-qcname">Gerätename (Optional)</label>
            <input id="input-qcname" type="text" placeholder="z.B. Mein Switch" bind:value={qcName} disabled={isConnecting} />
          </div>
          <div class="form-group">
            <label for="input-qcip">IP-Adresse / Host *</label>
            <input id="input-qcip" type="text" placeholder="z.B. 192.168.1.50" bind:value={qcIp} disabled={isConnecting} />
          </div>
          <div class="form-group">
            <label for="input-qcuser">Benutzername *</label>
            <input id="input-qcuser" type="text" placeholder="z.B. root" bind:value={qcUser} disabled={isConnecting} />
          </div>
          
          <div class="form-group">
            <label for="input-qcpass">Passwort</label>
            <div style="display: flex; gap: 5px;">
              <input id="input-qcpass" type="password" placeholder="••••••••" bind:value={qcPass} disabled={isConnecting} on:keydown={(e) => e.key === 'Enter' && handleConnect()} style="flex: 1;" />
              <button class="icon-btn refresh-btn" on:click={copyPassword} disabled={isConnecting || !qcPass} title="Passwort kopieren">📋</button>
            </div>
          </div>
        {:else}
          <div class="form-group">
            <label for="input-port">COM-Port *</label>
            <div style="display: flex; gap: 5px;">
              <select id="input-port" bind:value={selectedPort} disabled={isConnecting} style="flex: 1;">
                {#each serialPorts as port}<option value={port}>{port}</option>{/each}
              </select>
              <button class="icon-btn refresh-btn" on:click={loadSerialPorts} disabled={isConnecting} title="Ports aktualisieren">🔄</button>
            </div>
          </div>
          
          <div class="form-group">
            <label for="input-baud">Baudrate Profil</label>
            <div style="display: flex; gap: 5px;">
              <select id="input-baud" bind:value={selectedBaudLabel} disabled={isConnecting} style="flex: 1;">
                {#each baudProfiles as profile}
                  <option value={profile.label}>{profile.label}</option>
                {/each}
              </select>
              <button class="icon-btn refresh-btn" on:click={() => {newBaudName=''; newBaudRate=null; showBaudModal=true;}} disabled={isConnecting} title="Neues Profil hinzufügen">+</button>
              
              {#if !defaultProfiles.some(p => p.label === selectedBaudLabel)}
                <button class="icon-btn delete-baud-btn" on:click={deleteBaudProfile} disabled={isConnecting} title="Eigenes Profil löschen">-</button>
              {/if}
            </div>
          </div>
        {/if}

        <div class="button-row">
          <button class="action-btn connect-btn" on:click={handleConnect} disabled={isConnecting}>{isConnecting ? '...' : 'Verbinden 🚀'}</button>
          {#if mode === 'ssh'}
            <button class="action-btn save-btn" on:click={saveDevice} disabled={isConnecting}>Speichern 💾</button>
          {/if}
        </div>
      </div>

      <!-- UNTERER TEIL: Gespeicherte Geräte -->
      {#if mode === 'ssh'}
        <div class="saved-devices-container">
          <div class="section-title">GESPEICHERTE GERÄTE</div>
          <div class="devices-scroll-area">
            {#if savedHosts.length === 0}
              <div class="no-devices">Keine Geräte gespeichert.</div>
            {:else}
              {#each savedHosts as host}
                <div class="host-item">
                  <button class="host-btn" on:click={() => connectToSavedDevice(host)} disabled={isConnecting} title="In neuem Tab öffnen">
                    <div class="host-name">{#if host.type === 'serial'}🔌{:else}🌐{/if} {host.name || host.ip || host.port}</div>
                    <div class="host-ip">{#if host.type === 'serial'}{host.port} @ {host.baud} Baud{:else}{host.user}@{host.ip}{/if}</div>
                  </button>
                  <div class="host-actions">
                    <button class="icon-btn edit-btn" on:click={() => editDevice(host)} disabled={isConnecting} title="Bearbeiten">✏️</button>
                    <button class="icon-btn delete-btn" on:click={() => deleteDevice(host)} disabled={isConnecting} title="Löschen">❌</button>
                  </div>
                </div>
              {/each}
            {/if}
          </div>
        </div>
      {/if}
    </div>
  </nav>

  <!-- Rechter Bereich: TABS + TERMINAL + MAKROS -->
  <section class="main-content">
    
    <!-- Die Tab-Leiste -->
    {#if sessions.length > 0}
      <div class="tabs-header">
        {#each sessions as session (session.id)}
          <div class="tab {activeSessionId === session.id ? 'active' : ''}" 
               role="button" 
               tabindex="0"
               on:click={() => switchTab(session.id)}
               on:keydown={(e) => e.key === 'Enter' && switchTab(session.id)}>
            <span class="tab-icon">{session.type === 'ssh' ? '🌐' : '🔌'}</span>
            <span class="tab-title">{session.title}</span>
            <button class="tab-close" on:click={(e) => closeTab(session.id, e)}>×</button>
          </div>
        {/each}
      </div>
    {:else}
      <div class="no-tabs-placeholder">
        <div class="placeholder-text">Keine aktive Sitzung.<br/>Wähle ein Gerät oder einen Port aus.</div>
      </div>
    {/if}

    <!-- Terminal Container -->
    <div class="terminals-container">
      
      <!-- Suchleiste (Live-Suche) -->
      {#if showSearchBar}
        <div class="search-bar">
          <input 
            id="term-search-input" 
            type="text" 
            placeholder="Im Terminal suchen..." 
            bind:value={searchTerm} 
            on:input={() => triggerSearch(true)} 
            on:keydown={(e) => e.key === 'Enter' && triggerSearch(false)} 
          />
          <button class="icon-btn search-btn" on:click={findPrevious} title="Vorheriger">⬆️</button>
          <button class="icon-btn search-btn" on:click={() => triggerSearch(false)} title="Nächster (Enter)">⬇️</button>
          <div class="search-divider"></div>
          <button class="icon-btn search-close" on:click={closeSearch} title="Schließen (ESC)">❌</button>
        </div>
      {/if}

      {#each sessions as session (session.id)}
        <div id="term-{session.id}" class="xterm-wrapper" style="display: {activeSessionId === session.id ? 'block' : 'none'};"></div>
      {/each}
    </div>

    <!-- MAKRO / SNIPPET LEISTE -->
    <div class="snippets-toolbar">
      <span class="snippets-label">⚡ Makros:</span>
      <div class="snippets-list">
        {#each savedSnippets as snip}
          <div class="snippet-wrapper">
            <button class="snippet-btn" on:click={() => executeSnippet(snip.cmd)} title={snip.cmd}>{snip.name}</button>
            <button class="snippet-del" on:click={(e) => deleteSnippet(snip, e)} title="Makro löschen">×</button>
          </div>
        {/each}
      </div>
      <button class="add-snippet-btn" on:click={() => {newSnippetName=''; newSnippetCmd=''; showSnippetModal=true;}} title="Neues Makro erstellen">+</button>
    </div>

  </section>

  <!-- MODAL FÜR NEUE BAUDRATEN PROFILE -->
  {#if showBaudModal}
    <div class="modal-overlay">
      <div class="modal-content">
        <h3>Neues Baudraten-Profil</h3>
        <div class="form-group">
          <label for="input-newbaudname">Geräteklasse / Name (Optional)</label>
          <input id="input-newbaudname" type="text" placeholder="z.B. Cisco Switch" bind:value={newBaudName} on:keydown={(e) => e.key === 'Enter' && addBaudProfile()}/>
        </div>
        <div class="form-group">
          <label for="input-newbaudrate">Baudrate *</label>
          <input id="input-newbaudrate" type="number" placeholder="z.B. 115200" bind:value={newBaudRate} on:keydown={(e) => e.key === 'Enter' && addBaudProfile()}/>
        </div>
        <div class="button-row" style="margin-top: 25px;">
          <button class="action-btn disconnect-btn" on:click={() => showBaudModal=false}>Abbrechen</button>
          <button class="action-btn connect-btn" on:click={addBaudProfile}>Hinzufügen</button>
        </div>
      </div>
    </div>
  {/if}

  <!-- MODAL FÜR NEUE MAKROS -->
  {#if showSnippetModal}
    <div class="modal-overlay">
      <div class="modal-content">
        <h3>Neues Makro erstellen</h3>
        <div class="form-group">
          <label for="input-newsnippetname">Name des Makros *</label>
          <input id="input-newsnippetname" type="text" placeholder="z.B. Update Sys" bind:value={newSnippetName} />
        </div>
        <div class="form-group">
          <label for="input-newsnippetcmd">Befehl *</label>
          <textarea id="input-newsnippetcmd" placeholder="sudo apt update && sudo apt upgrade -y" bind:value={newSnippetCmd} rows="3"></textarea>
          <small style="color: #888; font-size: 0.7rem; margin-top: 5px;">Enter wird automatisch simuliert.</small>
        </div>
        <div class="button-row" style="margin-top: 25px;">
          <button class="action-btn disconnect-btn" on:click={() => showSnippetModal=false}>Abbrechen</button>
          <button class="action-btn connect-btn" on:click={addSnippet}>Speichern</button>
        </div>
      </div>
    </div>
  {/if}
</main>

<style>
  :global(::-webkit-scrollbar) { width: 8px; height: 8px; }
  :global(::-webkit-scrollbar-track) { background: transparent; }
  :global(::-webkit-scrollbar-thumb) { background: #2e2e3e; border-radius: 4px; }
  :global(::-webkit-scrollbar-thumb:hover) { background: #6200ee; }
  :global(body) { margin: 0; padding: 0; font-family: 'Segoe UI', sans-serif; background-color: #0f0f17; color: white; overflow: hidden; }
  :global(.xterm) { padding: 10px; text-align: left !important; }

  main { display: flex; height: 100vh; width: 100vw; }

  /* Sidebar Styling */
  .sidebar { width: 320px; background-color: #161621; border-right: 1px solid #2e2e3e; display: flex; flex-direction: column; z-index: 10;}
  .logo-area { padding: 25px 20px; font-size: 1.4rem; font-weight: 800; letter-spacing: 2px; border-bottom: 1px solid #2e2e3e; flex-shrink: 0; }
  .logo-text { color: white; } .logo-dot { color: #6200ee; }
  .sidebar-layout { display: flex; flex-direction: column; flex: 1; overflow: hidden; }
  .quick-connect-container { padding: 20px; flex-shrink: 0; }
  .saved-devices-container { display: flex; flex-direction: column; border-top: 1px solid #2e2e3e; flex: 1; overflow: hidden; }
  .section-title { font-size: 0.75rem; color: #888899; letter-spacing: 1px; margin-bottom: 15px; font-weight: bold; }
  .saved-devices-container .section-title { padding: 20px 20px 10px 20px; margin-bottom: 0; }
  .devices-scroll-area { padding: 10px 20px 20px 20px; flex: 1; overflow: hidden; overflow-y: auto; }

  /* Toggles, Form & Buttons */
  .mode-toggle { display: flex; background: #232333; border-radius: 6px; margin-bottom: 15px; padding: 4px; }
  .toggle-btn { flex: 1; padding: 8px; background: transparent; border: none; color: #888; border-radius: 4px; cursor: pointer; transition: 0.2s; font-size: 0.85rem; font-weight: bold; }
  .toggle-btn.active { background: #6200ee; color: white; }
  .form-group { margin-bottom: 12px; display: flex; flex-direction: column; }
  .form-group label { font-size: 0.8rem; color: #aaa; margin-bottom: 5px; }
  .form-group input, .form-group select, .form-group textarea { background: #232333; border: 1px solid #3a3a4e; color: white; padding: 8px 12px; border-radius: 6px; outline: none; font-family: 'Segoe UI', sans-serif; transition: border-color 0.2s; }
  .form-group textarea { resize: none; }
  .form-group input:focus, .form-group select:focus, .form-group textarea:focus { border-color: #6200ee; }
  .button-row { display: flex; gap: 10px; margin-top: 15px; }
  .action-btn { flex: 1; padding: 10px; border: none; border-radius: 6px; color: white; font-weight: bold; font-size: 0.9rem; cursor: pointer; transition: 0.2s; }
  .action-btn:active { transform: scale(0.98); } .action-btn:disabled { opacity: 0.5; cursor: not-allowed; }
  .connect-btn { background: #6200ee; } .connect-btn:hover:not(:disabled) { background: #7c22ff; }
  .save-btn { background: #2e2e45; color: #fff; border: 1px solid #3a3a4e; } .save-btn:hover:not(:disabled) { background: #3a3a5e; }
  .disconnect-btn { background: #e91e63; } .disconnect-btn:hover { background: #ff4081; }

  /* Host Liste */
  .no-devices { font-size: 0.85rem; color: #666; font-style: italic; }
  .host-item { display: flex; align-items: center; margin-bottom: 8px; background: #1e1e2d; border-radius: 6px; border: 1px solid transparent; transition: 0.2s; }
  .host-item:hover { border-color: #3a3a4e; }
  .host-btn { flex: 1; background: transparent; border: none; padding: 10px; text-align: left; cursor: pointer; color: white; }
  .host-name { font-size: 0.9rem; font-weight: 600; } .host-ip { font-size: 0.75rem; color: #888899; margin-top: 2px; }
  .host-actions { display: flex; padding-right: 5px; }
  .icon-btn { background: transparent; border: none; color: #888; font-size: 1rem; padding: 8px; cursor: pointer; border-radius: 4px; transition: 0.2s; }
  .refresh-btn { background: #2e2e45; border: 1px solid #3a3a4e; } .refresh-btn:hover:not(:disabled) { background: #3a3a5e; }
  .delete-baud-btn { background: #2e2e45; border: 1px solid #3a3a4e; color: #ff3366; font-weight: bold; }
  .delete-baud-btn:hover:not(:disabled) { background: #4a2e3e; border-color: #ff3366; }
  .edit-btn:hover:not(:disabled) { background: #2e2e45; color: white; } .delete-btn:hover:not(:disabled) { background: #ff336620; color: #ff3366; }

  /* --- TABS & HAUPTBEREICH --- */
  .main-content { flex: 1; display: flex; flex-direction: column; background-color: #0f0f17; }
  
  .tabs-header { display: flex; height: 42px; background: #161621; border-bottom: 1px solid #2e2e3e; overflow-x: auto; overflow-y: hidden; }
  .tabs-header::-webkit-scrollbar { height: 4px; } 

  .tab { display: flex; align-items: center; padding: 0 15px; min-width: 120px; max-width: 250px; background: #1a1a26; border-right: 1px solid #2e2e3e; cursor: pointer; transition: background 0.2s; border-top: 2px solid transparent; }
  .tab:hover { background: #232333; }
  .tab.active { background: #0f0f17; border-top-color: #6200ee; }
  .tab-icon { margin-right: 8px; font-size: 0.9rem; }
  .tab-title { flex: 1; font-size: 0.85rem; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; color: #ddd; }
  .tab.active .tab-title { color: white; font-weight: 600; }
  .tab-close { background: transparent; border: none; color: #888; font-size: 1.1rem; padding: 0 0 0 10px; cursor: pointer; margin-left: 5px; }
  .tab-close:hover { color: #ff3366; }

  .no-tabs-placeholder { flex: 1; display: flex; align-items: center; justify-content: center; }
  .placeholder-text { text-align: center; color: #555; font-size: 1.2rem; font-style: italic; border: 2px dashed #333; padding: 40px; border-radius: 20px;}

  .terminals-container { flex: 1; position: relative; }
  .xterm-wrapper { position: absolute; top: 0; left: 0; right: 0; bottom: 0; overflow: hidden; }

  /* --- SEARCH BAR STYLING --- */
  .search-bar {
    position: absolute; top: 15px; right: 25px; z-index: 50;
    background: #1a1a26; border: 1px solid #3a3a4e; border-radius: 8px;
    display: flex; align-items: center; padding: 5px;
    box-shadow: 0 4px 15px rgba(0,0,0,0.5);
  }
  .search-bar input {
    background: transparent; border: none; color: white;
    padding: 5px 10px; outline: none; width: 200px;
    font-family: 'Segoe UI', sans-serif;
  }
  .search-btn { font-size: 0.8rem; padding: 5px 8px; }
  .search-btn:hover { background: #3a3a4e; }
  .search-divider { width: 1px; height: 20px; background: #3a3a4e; margin: 0 5px; }
  .search-close { font-size: 0.8rem; padding: 5px 8px; color: #ff3366; }
  .search-close:hover { background: #ff336620; }

  /* --- MAKRO LEISTE --- */
  .snippets-toolbar {
    height: 45px; background: #161621; border-top: 1px solid #2e2e3e;
    display: flex; align-items: center; padding: 0 15px;
  }
  .snippets-label { font-size: 0.85rem; font-weight: bold; color: #888; margin-right: 15px; }
  .snippets-list { display: flex; flex: 1; overflow-x: auto; gap: 8px; align-items: center; }
  .snippets-list::-webkit-scrollbar { height: 4px; }
  
  .snippet-wrapper { display: flex; align-items: center; background: #232333; border-radius: 6px; border: 1px solid #2e2e3e; }
  .snippet-btn {
    background: transparent; border: none; color: white; font-size: 0.8rem;
    padding: 6px 12px; cursor: pointer; white-space: nowrap; font-family: 'Segoe UI', sans-serif;
  }
  .snippet-btn:hover { background: #3a3a4e; border-radius: 6px 0 0 6px; }
  .snippet-del {
    background: transparent; border: none; border-left: 1px solid #3a3a4e; color: #888; 
    font-size: 1rem; padding: 0 8px; cursor: pointer; transition: 0.2s;
  }
  .snippet-del:hover { color: #ff3366; background: #ff336620; border-radius: 0 6px 6px 0;}
  
  .add-snippet-btn {
    background: #2e2e45; border: 1px solid #3a3a4e; color: white; font-weight: bold;
    border-radius: 6px; padding: 5px 12px; cursor: pointer; margin-left: 10px; transition: 0.2s;
  }
  .add-snippet-btn:hover { background: #6200ee; border-color: #6200ee; }

  /* --- MODAL STYLING --- */
  .modal-overlay {
    position: fixed; top: 0; left: 0; right: 0; bottom: 0;
    background: rgba(0, 0, 0, 0.75); display: flex; align-items: center; justify-content: center;
    z-index: 1000; backdrop-filter: blur(4px);
  }
  .modal-content {
    background: #1a1a26; border: 1px solid #2e2e3e; border-radius: 12px; 
    padding: 30px; width: 350px; box-shadow: 0 15px 40px rgba(0,0,0,0.6);
  }
  .modal-content h3 { margin-top: 0; margin-bottom: 20px; font-size: 1.1rem; color: white; }
</style>