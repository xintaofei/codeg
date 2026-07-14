package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestStatusJSONShape(t *testing.T) {
	s := StatusResponse{State: "needs_login", LoginURL: "https://login.tailscale.com/a/x"}
	b, err := json.Marshal(s)
	if err != nil {
		t.Fatal(err)
	}
	var m map[string]any
	if err := json.Unmarshal(b, &m); err != nil {
		t.Fatal(err)
	}
	if m["state"] != "needs_login" {
		t.Fatalf("state=%v", m["state"])
	}
	if m["loginUrl"] != "https://login.tailscale.com/a/x" {
		t.Fatalf("loginUrl=%v", m["loginUrl"])
	}
}

func TestAuthMiddlewareRejectsMissingToken(t *testing.T) {
	h := withToken("secret", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(204)
	}))
	rr := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/status", nil)
	h.ServeHTTP(rr, req)
	if rr.Code != http.StatusUnauthorized {
		t.Fatalf("code=%d", rr.Code)
	}
}

func TestAuthMiddlewareAcceptsValidToken(t *testing.T) {
	h := withToken("secret", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(204)
	}))
	rr := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/status", nil)
	req.Header.Set("X-Codeg-Tsnet-Token", "secret")
	h.ServeHTTP(rr, req)
	if rr.Code != http.StatusNoContent {
		t.Fatalf("code=%d", rr.Code)
	}
}

func TestStatusHandlerReturnsStopped(t *testing.T) {
	mgr := newNodeManager("codeg-test", t.TempDir(), "")
	ctrl := &controlServer{mgr: mgr}
	h := withToken("tok", ctrl.routes())
	rr := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/status", nil)
	req.Header.Set("X-Codeg-Tsnet-Token", "tok")
	h.ServeHTTP(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("code=%d body=%s", rr.Code, rr.Body.String())
	}
	var st StatusResponse
	if err := json.Unmarshal(rr.Body.Bytes(), &st); err != nil {
		t.Fatal(err)
	}
	if st.State != stateStopped {
		t.Fatalf("state=%s", st.State)
	}
	if st.Hostname != "codeg-test" {
		t.Fatalf("hostname=%s", st.Hostname)
	}
}
