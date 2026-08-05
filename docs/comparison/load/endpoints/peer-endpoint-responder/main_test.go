package main

import "testing"

func TestDeterministicTagMatchesTheNeutralProfileVector(t *testing.T) {
	tag, ok := deterministicTag(
		7,
		"cl-0123456789abcdef0123456789abcdef-0@driver.invalid",
	)
	if !ok {
		t.Fatal("the profile call identifier was rejected")
	}
	if tag != "t-6ebec0059f8c0003" {
		t.Fatalf("tag = %q, want the profile vector", tag)
	}
}

func TestDeterministicTagRejectsIdentifiersOutsideTheProfile(t *testing.T) {
	invalid := []string{
		"",
		"ordinary-call@example.invalid",
		"cl-not-hex-0@driver.invalid",
		"cl-0123456789abcdef0123456789abcdef-x@driver.invalid",
	}
	for _, value := range invalid {
		if tag, ok := deterministicTag(7, value); ok {
			t.Fatalf("deterministicTag(%q) = %q, want rejection", value, tag)
		}
	}
}
