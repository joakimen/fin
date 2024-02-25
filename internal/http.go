package internal

import (
	"fmt"
	"io"
	"log"
	"net/http"
)

func NewRequest(method string, url string) *http.Request {
	req, err := http.NewRequest(method, url, http.NoBody)
	if err != nil {
		log.Fatal(err)
	}
	return req
}

func NewClient() *http.Client {
	return &http.Client{}
}

func DoRequest(client *http.Client, req *http.Request) []byte {
	resp, err := client.Do(req)
	if err != nil {
		log.Fatalf("request failed: %v", err)
	}
	body, err := io.ReadAll(resp.Body)
	defer resp.Body.Close()
	if err != nil {
		panic(fmt.Sprintf("couldn't read response body: %v", err))
	}
	return body
}
