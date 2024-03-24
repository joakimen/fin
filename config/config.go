package config

import (
	"flag"
	"fmt"
	"log"
	"os"
	"strconv"
	"time"
)

type Config struct {
	StartDate     time.Time
	SaveTestData  bool
	ReverseOutput bool

	Todoist struct {
		APIToken string
	}

	Jira struct {
		APIUser  string
		APIToken string
		APIHost  string
	}
}

func LoadConfig() *Config {
	var cfg Config
	err := loadFlags(&cfg)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error parsing flags:", err)
		os.Exit(1)
	}
	loadEnvVars(&cfg)
	return &cfg
}

// Load flags into config struct
func loadFlags(cfg *Config) error {
	daysBackP := flag.Int("d", 1, "days back to find completed task")
	cutoffTimeP := flag.String("c", "00:00", "cutoff time for completed tasks (HH:MM)")
	flag.StringVar(&cfg.Todoist.APIToken, "t", "", "todoist API token")
	flag.BoolVar(&cfg.SaveTestData, "s", false, "save downloaded task data to testdata/ directory")
	flag.BoolVar(&cfg.ReverseOutput, "r", false, "reverse the output order of tasks")
	flag.Parse()

	// deref since go doesn't have real referential transparency
	daysBack := *daysBackP
	cutoffTime := *cutoffTimeP

	now := time.Now()
	hours, err := strconv.Atoi(cutoffTime[0:2])
	if err != nil {
		return err
	}

	minutes, err := strconv.Atoi(cutoffTime[3:5])
	if err != nil {
		return err
	}
	startDate := time.Date(now.Year(), now.Month(), now.Day()-daysBack, hours, minutes, 0, 0, time.Local)

	if err != nil {
		fmt.Fprintf(os.Stderr, "error parsing cutoff time:", err)
		os.Exit(1)
	}
	cfg.StartDate = startDate
	return nil
}

// Load env vars into config struct
func loadEnvVars(cfg *Config) {
	envVars := []struct {
		cfgKey  *string
		envName string
	}{
		{&cfg.Todoist.APIToken, "TODOIST_TOKEN"},
		{&cfg.Jira.APIUser, "JIRA_API_USER"},
		{&cfg.Jira.APIToken, "JIRA_API_TOKEN"},
		{&cfg.Jira.APIHost, "JIRA_HOST"},
	}

	for _, envVar := range envVars {
		*envVar.cfgKey = getEnv(envVar.envName)
	}
}

func getEnv(key string) string {
	value := os.Getenv(key)
	if value == "" {
		log.Fatalln("missing required environment variable", key)
	}
	return value
}
