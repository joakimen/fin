package internal

import (
	"bytes"
	"encoding/json"
	"fmt"
	"log"
)

func Deserialize[T any](data []byte) T {
	var result T
	err := json.Unmarshal(data, &result)
	if err != nil {
		panic(fmt.Errorf("couldn't unmarshal JSON: %v", err))
	}
	return result
}

func Format(jsonData []byte) []byte {
	var prettyJSON bytes.Buffer
	prefix := ""
	indent := "  "
	if err := json.Indent(&prettyJSON, jsonData, prefix, indent); err != nil {
		log.Fatalf("failed to format JSON: %v", err)
	}
	return prettyJSON.Bytes()
}
