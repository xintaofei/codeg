package main

import (
	"encoding/json"
	"testing"
)

func TestBootstrapJSONShape(t *testing.T) {
	b := bootstrapInfo{ControlAddr: "127.0.0.1:9", PID: 42}
	raw, err := json.Marshal(b)
	if err != nil {
		t.Fatal(err)
	}
	var m map[string]any
	if err := json.Unmarshal(raw, &m); err != nil {
		t.Fatal(err)
	}
	if m["controlAddr"] != "127.0.0.1:9" {
		t.Fatalf("controlAddr=%v", m["controlAddr"])
	}
	if int(m["pid"].(float64)) != 42 {
		t.Fatalf("pid=%v", m["pid"])
	}
}
