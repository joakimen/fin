package config

import (
	"flag"
	"log"
	"os"
)

type Config struct {
	Days          int
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
	loadFlags(&cfg)
	loadEnvVars(&cfg)
	return &cfg
}

// Load flags into config struct
func loadFlags(cfg *Config) {
	flag.IntVar(&cfg.Days, "d", 1, "days back to find completed task")
	flag.BoolVar(&cfg.SaveTestData, "s", false, "save downloaded task data to testdata/ directory")
	flag.BoolVar(&cfg.ReverseOutput, "r", false, "reverse the output order of tasks")
	flag.Parse()
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
