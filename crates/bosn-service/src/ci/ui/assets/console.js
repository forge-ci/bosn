import RFB from "/console/novnc.js";

const $ = (id) => document.getElementById(id);
const id = location.pathname.split("/").pop();
let rfb = null;
let channel = null;
let transportError = false;
const errors = {
  certificate_changed: "The iDRAC certificate changed. Verify the server identity and update its trusted fingerprint before reconnecting.",
  console_unreachable: "Cannot reach the encrypted iDRAC VNC service. Check the server connection, VNC setting and console license.",
  console_closed: "The console connection ended. Check the VNC password and server availability, then reconnect.",
  console_connection_lost: "The console connection was lost or reached its time limit. You can reconnect.",
  docker_unavailable: "Docker is unavailable. Start the container engine, then reconnect.",
  console_start_failed: "Bosn could not start the console session. Try connecting again.",
};

function notice(message) {
  $("notice").textContent = message;
  $("notice").hidden = !message;
}

function state(message, connected = false, pending = false) {
  $("status").textContent = message;
  $("connect").disabled = connected || pending;
  $("disconnect").disabled = !connected && !pending;
  $("keys").disabled = !connected;
}

function disconnect() {
  if (rfb) rfb.disconnect();
  if (channel) channel.close();
  rfb = channel = null;
  $("credentials").hidden = true;
  $("password").value = "";
  state("Disconnected");
}

function connect() {
  disconnect();
  notice("");
  transportError = false;
  state("Starting encrypted console…", false, true);
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  channel = new WebSocket(`${scheme}//${location.host}/v1/consoles/${encodeURIComponent(id)}/socket`);
  channel.binaryType = "arraybuffer";
  // Control diagnostics are distinct from binary RFB bytes. Consume these
  // before noVNC's listener so an error can never be mistaken for a framebuffer.
  channel.addEventListener("message", (event) => {
    if (typeof event.data !== "string") return;
    event.stopImmediatePropagation();
    try {
      const message = JSON.parse(event.data);
      if (typeof message.console_error === "string") {
        transportError = true;
        notice(errors[message.console_error] || "The console connection failed.");
      }
    } catch { notice("The console transport returned an invalid response."); }
  });
  rfb = new RFB($("screen"), channel, { shared: true });
  rfb.scaleViewport = true;
  rfb.resizeSession = false;
  rfb.addEventListener("connect", () => state("Connected · encrypted iDRAC VNC", true));
  rfb.addEventListener("credentialsrequired", () => {
    $("credentials").hidden = false;
    $("password").focus();
    state("VNC password required", false, true);
  });
  rfb.addEventListener("securityfailure", () => {
    transportError = true;
    notice("The server refused VNC authentication. Check the separate iDRAC VNC password.");
  });
  rfb.addEventListener("disconnect", (event) => {
    $("credentials").hidden = true;
    $("password").value = "";
    state("Disconnected");
    if (!event.detail.clean && !transportError) notice("Could not connect. Check your Bosn session, server configuration and Docker engine.");
  });
}

$("connect").addEventListener("click", connect);
$("disconnect").addEventListener("click", disconnect);
$("keys").addEventListener("click", () => rfb?.sendCtrlAltDel());
$("credentials").addEventListener("submit", (event) => {
  event.preventDefault();
  rfb?.sendCredentials({ password: $("password").value });
  $("password").value = "";
  $("credentials").hidden = true;
  state("Authenticating…", false, true);
});
window.addEventListener("pagehide", disconnect);

try {
  const response = await fetch("/v1/consoles");
  if (!response.ok) throw new Error("Server configuration unavailable");
  const servers = await response.json();
  const server = servers.find((item) => item.id === id);
  if (!server) throw new Error("This server is no longer configured");
  $("name").textContent = server.name;
  document.title = `Bosn · ${server.name}`;
  connect();
} catch (error) {
  notice(error.message);
  $("connect").disabled = true;
}
