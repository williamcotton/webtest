package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"strings"
	"sync"
)

type Manifest struct {
	SchemaHash string         `json:"schema_hash"`
	Functions  map[string]any `json:"functions"`
}

var users = map[string]map[string]any{}
var lock sync.Mutex

func main() {
	if os.Getenv("WEBTEST") != "1" {
		panic("this example bridge is test-only")
	}
	bytes, err := os.ReadFile(".webtest/app-schema.json")
	if err != nil {
		panic(err)
	}
	var manifest Manifest
	if err = json.Unmarshal(bytes, &manifest); err != nil {
		panic(err)
	}
	server := &http.Server{Addr: "127.0.0.1:" + env("PORT", "3103"), Handler: routes()}
	go func() {
		if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			panic(err)
		}
	}()
	runBridge(manifest)
	_ = server.Close()
}
func routes() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("/health", func(w http.ResponseWriter, _ *http.Request) { _, _ = io.WriteString(w, "ok") })
	mux.HandleFunc("/login", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("content-type", "text/html")
		if r.Method == "POST" {
			_ = r.ParseForm()
			email := r.FormValue("email")
			lock.Lock()
			_, ok := users[email]
			lock.Unlock()
			if ok {
				_, _ = fmt.Fprintf(w, "<p>Welcome, %s</p>", email)
			} else {
				_, _ = io.WriteString(w, "<p>Invalid sign in</p>")
			}
			return
		}
		_, _ = io.WriteString(w, `<form method="post"><label>Email <input name="email"></label><button>Sign in</button></form>`)
	})
	return mux
}
func runBridge(manifest Manifest) {
	endpoint := os.Getenv("WEBTEST_BRIDGE")
	var conn net.Conn
	var err error
	if strings.HasPrefix(endpoint, "tcp://") {
		parsed, _ := url.Parse(endpoint)
		conn, err = net.Dial("tcp", parsed.Host)
	} else if strings.HasPrefix(endpoint, "unix:") {
		conn, err = net.Dial("unix", strings.TrimPrefix(endpoint, "unix:"))
	} else {
		panic("unsupported endpoint")
	}
	if err != nil {
		panic(err)
	}
	defer conn.Close()
	reader := bufio.NewReader(conn)
	send(conn, map[string]any{"type": "hello", "protocol_versions": []int{1}, "sdk": "webtest-go-example", "sdk_version": "0.1.0", "token": os.Getenv("WEBTEST_TOKEN"), "capabilities": map[string]bool{"cancel": false, "events": false}})
	var message map[string]any
	receive(reader, &message)
	if message["type"] != "hello_ok" {
		return
	}
	for {
		message = map[string]any{}
		if receive(reader, &message) != nil {
			return
		}
		id := message["id"]
		switch message["type"] {
		case "describe":
			send(conn, map[string]any{"type": "schema", "id": id, "protocol": 1, "schema_hash": manifest.SchemaHash, "functions": manifest.Functions})
		case "call":
			args := message["arguments"].(map[string]any)
			email := args["email"].(string)
			lock.Lock()
			_, exists := users[email]
			if exists {
				lock.Unlock()
				send(conn, map[string]any{"type": "error", "id": id, "code": "user.email_taken", "message": "email already exists", "retryable": false, "data": map[string]any{}})
			} else {
				admin, _ := args["admin"].(bool)
				user := map[string]any{"id": len(users) + 1, "email": email, "admin": admin}
				users[email] = user
				lock.Unlock()
				send(conn, map[string]any{"type": "result", "id": id, "value": user})
			}
		case "ping":
			send(conn, map[string]any{"type": "pong", "id": id})
		case "shutdown":
			send(conn, map[string]any{"type": "shutdown_ok", "id": id})
			return
		}
	}
}
func send(w io.Writer, value any) {
	bytes, _ := json.Marshal(value)
	_, _ = w.Write(append(bytes, '\n'))
}
func receive(r *bufio.Reader, value any) error {
	bytes, err := r.ReadBytes('\n')
	if err != nil {
		return err
	}
	return json.Unmarshal(bytes, value)
}
func env(name, fallback string) string {
	if value := os.Getenv(name); value != "" {
		return value
	}
	return fallback
}
