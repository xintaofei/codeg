package main

// StatusResponse is the control-plane status payload returned by GET /status.
type StatusResponse struct {
	State        string `json:"state"`
	LoginURL     string `json:"loginUrl,omitempty"`
	FunnelURL    string `json:"funnelUrl,omitempty"`
	Hostname     string `json:"hostname,omitempty"`
	IPv4         string `json:"ipv4,omitempty"`
	LastError    string `json:"lastError,omitempty"`
	ErrorKey     string `json:"errorKey,omitempty"`
	BackendState string `json:"backendState,omitempty"`
}

const (
	stateStopped        = "stopped"
	stateStarting       = "starting"
	stateNeedsLogin     = "needs_login"
	stateConnecting     = "connecting"
	stateOnline         = "online"
	stateFunnelEnabling = "funnel_enabling"
	stateFunnelReady    = "funnel_ready"
	stateError          = "error"
	stateStopping       = "stopping"
)
