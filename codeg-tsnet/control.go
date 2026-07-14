package main

import (
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"time"
)

type bootstrapInfo struct {
	ControlAddr string `json:"controlAddr"`
	PID         int    `json:"pid"`
}

type upRequest struct {
	AuthKey string `json:"authKey,omitempty"`
}

type funnelRequest struct {
	Enabled       bool `json:"enabled"`
	LocalhostPort int  `json:"localhostPort"`
}

func withToken(token string, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if token == "" {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		got := r.Header.Get("X-Codeg-Tsnet-Token")
		if got == "" || got != token {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		next.ServeHTTP(w, r)
	})
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	enc := json.NewEncoder(w)
	enc.SetEscapeHTML(false)
	_ = enc.Encode(v)
}

func readJSONBody(r *http.Request, dst any) error {
	defer r.Body.Close()
	dec := json.NewDecoder(io.LimitReader(r.Body, 1<<20))
	dec.DisallowUnknownFields()
	if err := dec.Decode(dst); err != nil && !errors.Is(err, io.EOF) {
		return err
	}
	return nil
}

type controlServer struct {
	mgr      *nodeManager
	shutdown func()
}

func (c *controlServer) routes() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /status", c.handleStatus)
	mux.HandleFunc("POST /up", c.handleUp)
	mux.HandleFunc("POST /funnel", c.handleFunnel)
	mux.HandleFunc("POST /logout", c.handleLogout)
	mux.HandleFunc("POST /shutdown", c.handleShutdown)
	return mux
}

func (c *controlServer) handleStatus(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, c.mgr.snapshot())
}

func (c *controlServer) handleUp(w http.ResponseWriter, r *http.Request) {
	var req upRequest
	if err := readJSONBody(r, &req); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	if err := c.mgr.up(req.AuthKey); err != nil {
		http.Error(w, err.Error(), http.StatusConflict)
		return
	}
	writeJSON(w, http.StatusOK, c.mgr.snapshot())
}

func (c *controlServer) handleFunnel(w http.ResponseWriter, r *http.Request) {
	var req funnelRequest
	if err := readJSONBody(r, &req); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	var err error
	if req.Enabled {
		err = c.mgr.enableFunnel(req.LocalhostPort)
	} else {
		err = c.mgr.disableFunnel()
	}
	if err != nil {
		// Status already updated with errorKey; still return JSON status for controller.
		writeJSON(w, http.StatusOK, c.mgr.snapshot())
		return
	}
	writeJSON(w, http.StatusOK, c.mgr.snapshot())
}

func (c *controlServer) handleLogout(w http.ResponseWriter, r *http.Request) {
	if err := c.mgr.logout(); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	writeJSON(w, http.StatusOK, c.mgr.snapshot())
}

func (c *controlServer) handleShutdown(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, map[string]string{"ok": "true"})
	go func() {
		time.Sleep(50 * time.Millisecond)
		if c.shutdown != nil {
			c.shutdown()
		}
	}()
}
