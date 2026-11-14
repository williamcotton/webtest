const $ = (selector) => document.querySelector(selector);
const room = new URLSearchParams(location.search).get("room") ?? "lobby";
let socket;
let name;
const seen = new Set();

function connect(identity) {
  name = identity.name;
  $("#identity").textContent = `Signed in as ${name}`;
  $("#room-name").textContent = identity.room;
  $("#login").hidden = true;
  $("#conversation").hidden = false;
  $("#connection").textContent = "Connecting…";
  socket = new WebSocket(
    `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/chat?room=${encodeURIComponent(room)}`,
  );
  socket.addEventListener("message", (event) => {
    const value = JSON.parse(event.data);
    if (value.type === "ready") {
      $("#connection").textContent = "Connected";
      $("#connection").className = "connected";
      $("#send").disabled = false;
    } else if (value.type === "presence") {
      for (const person of ["Alice", "Bob"]) {
        const online = value.members.includes(person);
        $(`#${person.toLowerCase()}-status`).textContent =
          `${person} is ${online ? "online" : "offline"}`;
        $(`#${person.toLowerCase()}-dot`).className = online ? "online" : "";
      }
    } else if (value.type === "message" && !seen.has(value.id)) {
      seen.add(value.id);
      $("#empty").hidden = true;
      const item = document.createElement("li");
      item.className = value.from === name ? "mine" : "theirs";
      const author = document.createElement("span");
      author.className = "author";
      author.textContent = value.from;
      const body = document.createElement("p");
      body.textContent = value.body;
      item.append(author, body);
      $("#messages").append(item);
      item.scrollIntoView({ block: "nearest" });
    }
  });
  socket.addEventListener("error", () =>
    console.error("Chat connection failed"),
  );
  socket.addEventListener("close", () => {
    $("#connection").textContent = "Disconnected";
    $("#connection").className = "";
    $("#send").disabled = true;
  });
}
$("#login-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const data = new FormData(event.currentTarget);
  try {
    const response = await fetch("/api/login", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        room,
        email: data.get("email"),
        password: data.get("password"),
      }),
    });
    const identity = await response.json();
    if (!response.ok) {
      $("#login-error").textContent = identity.error;
      return;
    }
    $("#login-error").textContent = "";
    connect(identity);
  } catch {
    $("#login-error").textContent = "Unable to connect. Please try again.";
  }
});
$("#message-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const body = $("#message").value.trim();
  if (body && socket?.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify({ type: "send", body }));
    $("#message").value = "";
    $("#message").focus();
  }
});
try {
  const response = await fetch(`/api/session?room=${encodeURIComponent(room)}`);
  const identity = await response.json();
  if (!response.ok) $("#login-error").textContent = identity.error;
  else if (identity.name) connect(identity);
  else $("#room-name").textContent = identity.room;
} catch {
  $("#login-error").textContent = "Unable to load this room.";
}
window.addEventListener("pagehide", () => socket?.close());
