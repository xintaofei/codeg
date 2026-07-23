package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"strings"
	"sync"
	"time"

	"tailscale.com/client/local"
	"tailscale.com/ipn"
	"tailscale.com/tsnet"
)

type nodeManager struct {
	mu sync.Mutex

	hostname string
	stateDir string
	authKey  string

	state         string
	loginURL      string
	funnelURL     string
	ipv4          string
	lastError     string
	errorKey      string
	backendState  string
	localPort     int
	desiredFunnel bool

	server      *tsnet.Server
	funnelLn    net.Listener
	funnelSrv   *http.Server
	watchCancel context.CancelFunc
	upCancel    context.CancelFunc
}

func newNodeManager(hostname, stateDir, authKey string) *nodeManager {
	return &nodeManager{
		hostname: hostname,
		stateDir: stateDir,
		authKey:  authKey,
		state:    stateStopped,
	}
}

func (m *nodeManager) snapshot() StatusResponse {
	m.mu.Lock()
	defer m.mu.Unlock()
	return StatusResponse{
		State:        m.state,
		LoginURL:     m.loginURL,
		FunnelURL:    m.funnelURL,
		Hostname:     m.hostname,
		IPv4:         m.ipv4,
		LastError:    m.lastError,
		ErrorKey:     m.errorKey,
		BackendState: m.backendState,
	}
}

func (m *nodeManager) setError(key, msg string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.state = stateError
	m.errorKey = key
	m.lastError = msg
}

func (m *nodeManager) setState(state string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.state = state
}

func (m *nodeManager) ensureServerLocked() *tsnet.Server {
	if m.server != nil {
		return m.server
	}
	s := &tsnet.Server{
		Dir:      m.stateDir,
		Hostname: m.hostname,
		AuthKey:  m.authKey,
		UserLogf: func(format string, args ...any) {
			msg := fmt.Sprintf(format, args...)
			// Never log secrets. Auth keys and control tokens must not appear.
			lower := strings.ToLower(msg)
			if strings.Contains(lower, "authkey") || strings.Contains(lower, "auth key") {
				return
			}
			log.Printf("[tsnet] %s", msg)
		},
	}
	m.server = s
	return s
}

func (m *nodeManager) up(authKey string) error {
	m.mu.Lock()
	if m.state == stateStopping {
		m.mu.Unlock()
		return errors.New("node is stopping")
	}
	if authKey != "" {
		m.authKey = authKey
		if m.server != nil {
			m.server.AuthKey = authKey
		}
	}
	m.state = stateStarting
	m.loginURL = ""
	m.lastError = ""
	m.errorKey = ""
	m.backendState = ""
	if m.upCancel != nil {
		m.upCancel()
		m.upCancel = nil
	}
	if m.watchCancel != nil {
		m.watchCancel()
		m.watchCancel = nil
	}
	s := m.ensureServerLocked()
	ctx, cancel := context.WithCancel(context.Background())
	m.upCancel = cancel
	m.mu.Unlock()

	go m.runUp(ctx, s)
	return nil
}

func (m *nodeManager) runUp(ctx context.Context, s *tsnet.Server) {
	m.setState(stateConnecting)

	lc, err := s.LocalClient()
	if err != nil {
		m.setError("tailscale.start_failed", err.Error())
		return
	}

	watchCtx, watchCancel := context.WithCancel(ctx)
	m.mu.Lock()
	m.watchCancel = watchCancel
	m.mu.Unlock()

	go m.watchIPN(watchCtx, lc)

	// Kick interactive login when no auth key is configured so BrowseToURL / AuthURL appear.
	if strings.TrimSpace(s.AuthKey) == "" {
		if err := lc.StartLoginInteractive(ctx); err != nil {
			// Non-fatal: node may already be logged in or will surface NeedsLogin via bus.
			log.Printf("[tsnet] StartLoginInteractive: %v", err)
		}
	}

	ticker := time.NewTicker(500 * time.Millisecond)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			st, err := lc.Status(ctx)
			if err != nil {
				if ctx.Err() != nil {
					return
				}
				continue
			}
			m.mu.Lock()
			m.backendState = st.BackendState
			if st.AuthURL != "" {
				m.loginURL = st.AuthURL
				if m.state != stateFunnelEnabling && m.state != stateFunnelReady && m.state != stateOnline {
					m.state = stateNeedsLogin
				}
			}
			if len(st.TailscaleIPs) > 0 {
				m.ipv4 = st.TailscaleIPs[0].String()
			}
			backend := st.BackendState
			m.mu.Unlock()

			switch backend {
			case ipn.Running.String():
				m.mu.Lock()
				m.state = stateOnline
				m.loginURL = ""
				if len(st.TailscaleIPs) > 0 {
					m.ipv4 = st.TailscaleIPs[0].String()
				}
				desired := m.desiredFunnel
				port := m.localPort
				m.mu.Unlock()
				if desired && port > 0 {
					_ = m.enableFunnel(port)
				}
				return
			case ipn.NeedsLogin.String(), ipn.NeedsMachineAuth.String():
				m.mu.Lock()
				if st.AuthURL != "" {
					m.loginURL = st.AuthURL
				}
				m.state = stateNeedsLogin
				m.mu.Unlock()
			case ipn.Starting.String():
				m.setState(stateConnecting)
			}
		}
	}
}

func (m *nodeManager) watchIPN(ctx context.Context, lc *local.Client) {
	watcher, err := lc.WatchIPNBus(ctx, ipn.NotifyInitialState|ipn.NotifyNoPrivateKeys)
	if err != nil {
		log.Printf("[tsnet] WatchIPNBus: %v", err)
		return
	}
	defer watcher.Close()

	for {
		n, err := watcher.Next()
		if err != nil {
			if ctx.Err() != nil {
				return
			}
			log.Printf("[tsnet] IPN bus closed: %v", err)
			return
		}
		if n.ErrMessage != nil {
			m.setError("tailscale.start_failed", *n.ErrMessage)
			continue
		}
		m.mu.Lock()
		if n.BrowseToURL != nil && *n.BrowseToURL != "" {
			m.loginURL = *n.BrowseToURL
			if m.state != stateFunnelEnabling && m.state != stateFunnelReady && m.state != stateOnline {
				m.state = stateNeedsLogin
			}
		}
		if n.State != nil {
			m.backendState = n.State.String()
			switch *n.State {
			case ipn.NeedsLogin, ipn.NeedsMachineAuth:
				m.state = stateNeedsLogin
			case ipn.Starting:
				if m.state != stateFunnelEnabling && m.state != stateFunnelReady {
					m.state = stateConnecting
				}
			case ipn.Running:
				if m.state != stateFunnelEnabling && m.state != stateFunnelReady {
					m.state = stateOnline
				}
				m.loginURL = ""
			}
		}
		m.mu.Unlock()
	}
}

func (m *nodeManager) enableFunnel(localhostPort int) error {
	if localhostPort <= 0 || localhostPort > 65535 {
		return fmt.Errorf("invalid localhostPort %d", localhostPort)
	}

	m.mu.Lock()
	m.localPort = localhostPort
	m.desiredFunnel = true
	m.state = stateFunnelEnabling
	m.lastError = ""
	m.errorKey = ""
	s := m.ensureServerLocked()
	oldLn := m.funnelLn
	oldSrv := m.funnelSrv
	m.funnelLn = nil
	m.funnelSrv = nil
	m.mu.Unlock()

	if oldSrv != nil {
		_ = oldSrv.Close()
	}
	if oldLn != nil {
		_ = oldLn.Close()
	}

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	st, err := s.Up(ctx)
	if err != nil {
		lc, lcErr := s.LocalClient()
		if lcErr == nil {
			if cur, stErr := lc.Status(context.Background()); stErr == nil {
				m.mu.Lock()
				m.backendState = cur.BackendState
				if cur.AuthURL != "" {
					m.loginURL = cur.AuthURL
					m.state = stateNeedsLogin
					m.mu.Unlock()
					return nil
				}
				m.mu.Unlock()
			}
		}
		key := "tailscale.funnel_failed"
		msg := err.Error()
		lower := strings.ToLower(msg)
		if strings.Contains(lower, "access") || strings.Contains(lower, "funnel") {
			key = "tailscale.funnel_denied"
		}
		m.setError(key, msg)
		return err
	}

	m.mu.Lock()
	if len(st.TailscaleIPs) > 0 {
		m.ipv4 = st.TailscaleIPs[0].String()
	}
	m.state = stateFunnelEnabling
	m.mu.Unlock()

	ln, err := s.ListenFunnel("tcp", ":443")
	if err != nil {
		key := "tailscale.funnel_failed"
		msg := err.Error()
		lower := strings.ToLower(msg)
		if strings.Contains(lower, "access") || strings.Contains(lower, "not permitted") || strings.Contains(lower, "denied") {
			key = "tailscale.funnel_denied"
		}
		m.setError(key, msg)
		return err
	}

	target, _ := url.Parse(fmt.Sprintf("http://127.0.0.1:%d", localhostPort))
	proxy := httputil.NewSingleHostReverseProxy(target)
	originalDirector := proxy.Director
	proxy.Director = func(req *http.Request) {
		originalDirector(req)
		req.Host = target.Host
		req.URL.Scheme = target.Scheme
		req.URL.Host = target.Host
	}
	proxy.ErrorHandler = func(w http.ResponseWriter, r *http.Request, e error) {
		http.Error(w, "upstream unavailable", http.StatusBadGateway)
	}

	srv := &http.Server{Handler: proxy}
	domains := s.CertDomains()
	funnelURL := ""
	if len(domains) > 0 {
		funnelURL = "https://" + domains[0]
	}

	m.mu.Lock()
	m.funnelLn = ln
	m.funnelSrv = srv
	m.funnelURL = funnelURL
	m.state = stateFunnelReady
	m.lastError = ""
	m.errorKey = ""
	m.mu.Unlock()

	go func() {
		if err := srv.Serve(ln); err != nil && !errors.Is(err, http.ErrServerClosed) && !errors.Is(err, net.ErrClosed) {
			m.setError("tailscale.funnel_failed", err.Error())
		}
	}()
	return nil
}

func (m *nodeManager) disableFunnel() error {
	m.mu.Lock()
	m.desiredFunnel = false
	oldLn := m.funnelLn
	oldSrv := m.funnelSrv
	m.funnelLn = nil
	m.funnelSrv = nil
	m.funnelURL = ""
	s := m.server
	if m.state == stateFunnelReady || m.state == stateFunnelEnabling {
		if m.backendState == ipn.Running.String() || m.ipv4 != "" {
			m.state = stateOnline
		} else if m.loginURL != "" {
			m.state = stateNeedsLogin
		} else if m.server != nil {
			m.state = stateConnecting
		} else {
			m.state = stateStopped
		}
	}
	m.mu.Unlock()

	if oldSrv != nil {
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		_ = oldSrv.Shutdown(ctx)
		cancel()
		_ = oldSrv.Close()
	}
	if oldLn != nil {
		_ = oldLn.Close()
	}

	if s != nil {
		if lc, err := s.LocalClient(); err == nil {
			_ = lc.SetServeConfig(context.Background(), new(ipn.ServeConfig))
		}
	}
	return nil
}

func (m *nodeManager) logout() error {
	_ = m.disableFunnel()
	m.mu.Lock()
	s := m.server
	m.mu.Unlock()
	if s == nil {
		return nil
	}
	lc, err := s.LocalClient()
	if err != nil {
		return err
	}
	if err := lc.Logout(context.Background()); err != nil {
		return err
	}
	m.mu.Lock()
	m.loginURL = ""
	m.ipv4 = ""
	m.funnelURL = ""
	m.backendState = ""
	m.state = stateNeedsLogin
	m.mu.Unlock()
	return nil
}

func (m *nodeManager) shutdown() {
	m.mu.Lock()
	m.state = stateStopping
	if m.upCancel != nil {
		m.upCancel()
		m.upCancel = nil
	}
	if m.watchCancel != nil {
		m.watchCancel()
		m.watchCancel = nil
	}
	m.mu.Unlock()

	_ = m.disableFunnel()

	m.mu.Lock()
	s := m.server
	m.server = nil
	m.mu.Unlock()
	if s != nil {
		_ = s.Close()
	}

	m.mu.Lock()
	m.state = stateStopped
	m.loginURL = ""
	m.funnelURL = ""
	m.ipv4 = ""
	m.backendState = ""
	m.mu.Unlock()
}
