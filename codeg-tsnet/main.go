// Userspace Tailscale node bundled with Codeg.
// Joins the tailnet inside this process. No Tailscale app on the PC.
// Proxies HTTPS on the tailnet (or Funnel if --public) to a loopback target.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"tailscale.com/ipn/ipnstate"
	"tailscale.com/tsnet"
)

type event struct {
	Event   string `json:"event"`
	URL     string `json:"url,omitempty"`
	Mode    string `json:"mode,omitempty"`
	Message string `json:"message,omitempty"`
}

func emit(e event) {
	b, err := json.Marshal(e)
	if err != nil {
		return
	}
	fmt.Println(string(b))
}

func main() {
	target := flag.String("target", "http://127.0.0.1:3080", "loopback Codeg Web Service")
	hostname := flag.String("hostname", "codeg", "tailnet hostname")
	stateDir := flag.String("state-dir", "", "persistent tsnet state directory")
	public := flag.Bool("public", false, "Funnel (public HTTPS). Default is private tailnet only")
	flag.Parse()

	parsed, err := url.Parse(*target)
	if err != nil || parsed.Scheme != "http" {
		emit(event{Event: "error", Message: "target must be http://127.0.0.1:<port>"})
		os.Exit(2)
	}
	host := parsed.Hostname()
	if host != "127.0.0.1" && host != "localhost" {
		emit(event{Event: "error", Message: "target must be loopback"})
		os.Exit(2)
	}

	dir := strings.TrimSpace(*stateDir)
	if dir == "" {
		cfg, err := os.UserConfigDir()
		if err != nil {
			emit(event{Event: "error", Message: "cannot resolve config dir"})
			os.Exit(2)
		}
		dir = filepath.Join(cfg, "app.codeg", "tsnet")
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		emit(event{Event: "error", Message: "cannot create state dir"})
		os.Exit(2)
	}

	mode := "private"
	if *public {
		mode = "public"
	}

	srv := &tsnet.Server{
		Hostname: strings.TrimSpace(*hostname),
		Dir:      dir,
		UserLogf: func(format string, args ...any) {
			msg := fmt.Sprintf(format, args...)
			if u := authURLFromLog(msg); u != "" {
				emit(event{Event: "auth_url", URL: u})
			}
		},
		Logf: func(string, ...any) {},
	}
	defer srv.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	st, err := srv.Up(ctx)
	cancel()
	if err != nil {
		emit(event{Event: "error", Message: "tailnet login failed"})
		os.Exit(1)
	}

	if *public {
		funnelLn, err := srv.ListenFunnel("tcp", ":443")
		if err != nil {
			emit(event{Event: "error", Message: "could not start public Funnel listener"})
			os.Exit(1)
		}
		go http.Serve(funnelLn, reverseProxy(parsed))
	} else {
		tlsLn, err := srv.ListenTLS("tcp", ":443")
		if err != nil {
			emit(event{Event: "error", Message: "could not start private TLS listener"})
			os.Exit(1)
		}
		go http.Serve(tlsLn, reverseProxy(parsed))
	}

	emit(event{Event: "ready", URL: httpsURL(st), Mode: mode})

	stop := make(chan os.Signal, 1)
	signal.Notify(stop, os.Interrupt, syscall.SIGTERM)
	<-stop
}

func reverseProxy(target *url.URL) http.Handler {
	proxy := httputil.NewSingleHostReverseProxy(target)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		r.Host = target.Host
		proxy.ServeHTTP(w, r)
	})
}

func httpsURL(st *ipnstate.Status) string {
	if st == nil {
		return ""
	}
	if len(st.CertDomains) > 0 {
		return "https://" + strings.TrimSuffix(st.CertDomains[0], ".")
	}
	if st.Self != nil && st.Self.DNSName != "" {
		return "https://" + strings.TrimSuffix(st.Self.DNSName, ".")
	}
	return ""
}

func authURLFromLog(msg string) string {
	for _, part := range strings.Fields(msg) {
		if strings.HasPrefix(part, "https://login.tailscale.com/") {
			return strings.TrimRight(part, ".,)")
		}
	}
	return ""
}
