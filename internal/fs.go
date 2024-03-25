package internal

import (
	"fmt"
	"log"
	"log/slog"
	"os"
	"path/filepath"
)

func writeFile(path string, contents []byte) {
	if err := os.MkdirAll(filepath.Dir(path), os.ModePerm); err != nil {
		log.Fatalf("failed to create directory: %v", err)
	}

	if err := os.WriteFile(path, contents, 0o600); err != nil {
		log.Fatalf("failed to write file: %v", err)
	}
}

func WriteTestData(filename string, jsonData []byte) {
	path := filepath.Join("testdata", filename)
	sizeInKB := len(jsonData) / 1024
	slog.Debug(fmt.Sprintf("writing %dKB to %s\n", sizeInKB, path))
	writeFile(path, Format(jsonData))
}
