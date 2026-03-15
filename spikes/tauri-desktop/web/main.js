import { invoke } from "@tauri-apps/api/core";

const state = {
  tab: "onboarding",
  wayfarerId: "",
  status: "Spike ready",
  contacts: [
    { id: "5d0e3f8f3f6b5e2afc2b0792d9c6170ff1ab2247116c922bdb17b0694f17dc11", alias: "Astra" },
    { id: "6649d76efbd2e7e57e5e84b83395ceecf19d5dfa39d9f2d6b52b6be995f75218", alias: "Relay Lab" }
  ],
  selectedContact: "",
  messages: {
    "5d0e3f8f3f6b5e2afc2b0792d9c6170ff1ab2247116c922bdb17b0694f17dc11": [
      { dir: "in", text: "Hey, testing the Tauri spike build." },
      { dir: "out", text: "Looks good so far. Cross-platform shell is up." }
    ]
  },
  diagnostics: null
};

state.selectedContact = state.contacts[0]?.id || "";

const tabs = document.getElementById("tabs");
const panel = document.getElementById("panel");

tabs.addEventListener("click", (event) => {
  const btn = event.target.closest("button[data-tab]");
  if (!btn) return;
  state.tab = btn.dataset.tab;
  render();
});

function tinyId(id) {
  if (!id) return "(none)";
  return `${id.slice(0, 10)}...${id.slice(-6)}`;
}

function currentThread() {
  return state.messages[state.selectedContact] || [];
}

async function generateWayfarerId() {
  try {
    const id = await invoke("generate_wayfarer_id_mock");
    state.wayfarerId = id;
    state.status = `Generated Wayfarer ID ${tinyId(id)}`;
  } catch (err) {
    state.status = `ID generation failed: ${String(err)}`;
  }
  render();
}

async function loadDiagnostics() {
  try {
    state.diagnostics = await invoke("app_diagnostics");
    state.status = "Loaded backend diagnostics";
  } catch (err) {
    state.status = `Diagnostics failed: ${String(err)}`;
  }
  render();
}

function saveContact(form) {
  const id = form.get("contact_id")?.trim();
  const alias = form.get("contact_alias")?.trim() || "Unnamed";
  if (!id || !/^[0-9a-f]{64}$/.test(id)) {
    state.status = "Contact ID must be 64 lowercase hex characters";
    render();
    return;
  }
  const existing = state.contacts.find((c) => c.id === id);
  if (existing) {
    existing.alias = alias;
    state.status = `Updated contact ${alias}`;
  } else {
    state.contacts.unshift({ id, alias });
    state.status = `Added contact ${alias}`;
  }
  state.selectedContact = id;
  if (!state.messages[id]) state.messages[id] = [];
  render();
}

function sendMessage(form) {
  const body = form.get("body")?.trim();
  if (!state.selectedContact) {
    state.status = "Select a contact first";
    render();
    return;
  }
  if (!body) {
    state.status = "Message body cannot be empty";
    render();
    return;
  }
  const thread = state.messages[state.selectedContact] || [];
  thread.push({ dir: "out", text: body });
  state.messages[state.selectedContact] = thread;
  state.status = "Message queued (mock)";
  render();
}

function onboardingView() {
  return `
    <h2 class="section-title">Onboarding</h2>
    <div class="grid">
      <section class="card">
        <h3>Identity</h3>
        <p>Generate a mock Wayfarer ID using a Rust backend command.</p>
        <div class="row">
          <button class="cta" id="gen-id-btn">Generate Wayfarer ID</button>
          <button class="ghost" id="diag-btn">Load Diagnostics</button>
        </div>
        <p><strong>Current ID:</strong> ${state.wayfarerId || "Not generated"}</p>
      </section>
      <section class="card">
        <h3>Backend Diagnostics</h3>
        <pre>${state.diagnostics ? JSON.stringify(state.diagnostics, null, 2) : "No diagnostics loaded"}</pre>
      </section>
    </div>
  `;
}

function chatsView() {
  const contactOptions = state.contacts
    .map((c) => `<option value="${c.id}" ${c.id === state.selectedContact ? "selected" : ""}>${c.alias} (${tinyId(c.id)})</option>`)
    .join("");
  const bubbles = currentThread()
    .map((m) => `<div class="chat-bubble ${m.dir}">${m.text}</div>`)
    .join("");
  return `
    <h2 class="section-title">Chats</h2>
    <div class="grid">
      <section class="card">
        <label for="chat-contact">Selected contact</label>
        <select id="chat-contact">${contactOptions || "<option>No contacts</option>"}</select>
        <div id="thread">${bubbles || "<p>No messages yet.</p>"}</div>
      </section>
      <section class="card">
        <form id="send-form">
          <label for="body">Message body</label>
          <textarea id="body" name="body" rows="6" placeholder="Type a message..."></textarea>
          <div class="row">
            <button class="cta" type="submit">Send (mock)</button>
          </div>
        </form>
      </section>
    </div>
  `;
}

function contactsView() {
  const rows = state.contacts
    .map((c) => `<li><strong>${c.alias}</strong><br /><small>${c.id}</small></li>`)
    .join("");

  return `
    <h2 class="section-title">Contacts</h2>
    <div class="grid">
      <section class="card">
        <h3>Contact list</h3>
        <ul class="contacts-list">${rows || "<li>No contacts</li>"}</ul>
      </section>
      <section class="card">
        <h3>Add or update</h3>
        <form id="contact-form">
          <label for="contact_id">Wayfarer ID</label>
          <input id="contact_id" name="contact_id" placeholder="64 lowercase hex chars" />
          <label for="contact_alias">Display name</label>
          <input id="contact_alias" name="contact_alias" placeholder="Astra" />
          <div class="row">
            <button class="cta" type="submit">Save contact</button>
          </div>
        </form>
      </section>
    </div>
  `;
}

function settingsView() {
  return `
    <h2 class="section-title">Settings</h2>
    <div class="card">
      <p>This spike intentionally keeps settings lightweight.</p>
      <ul class="threads-list">
        <li>Target: evaluate cross-platform shell and packaging path.</li>
        <li>Backend bridge: Tauri command invocation is active.</li>
        <li>UI state: local-only mock data for flow validation.</li>
      </ul>
    </div>
  `;
}

function attachHandlers() {
  const gen = document.getElementById("gen-id-btn");
  if (gen) gen.addEventListener("click", generateWayfarerId);

  const diag = document.getElementById("diag-btn");
  if (diag) diag.addEventListener("click", loadDiagnostics);

  const contactForm = document.getElementById("contact-form");
  if (contactForm) {
    contactForm.addEventListener("submit", (event) => {
      event.preventDefault();
      saveContact(new FormData(contactForm));
    });
  }

  const sendForm = document.getElementById("send-form");
  if (sendForm) {
    sendForm.addEventListener("submit", (event) => {
      event.preventDefault();
      sendMessage(new FormData(sendForm));
      sendForm.reset();
    });
  }

  const picker = document.getElementById("chat-contact");
  if (picker) {
    picker.addEventListener("change", (event) => {
      state.selectedContact = event.target.value;
      render();
    });
  }
}

function render() {
  for (const tabButton of tabs.querySelectorAll(".tab")) {
    tabButton.classList.toggle("is-active", tabButton.dataset.tab === state.tab);
  }

  switch (state.tab) {
    case "onboarding":
      panel.innerHTML = onboardingView();
      break;
    case "chats":
      panel.innerHTML = chatsView();
      break;
    case "contacts":
      panel.innerHTML = contactsView();
      break;
    case "settings":
      panel.innerHTML = settingsView();
      break;
    default:
      panel.innerHTML = onboardingView();
  }

  panel.insertAdjacentHTML("beforeend", `<div class="status">${state.status}</div>`);
  attachHandlers();
}

render();
