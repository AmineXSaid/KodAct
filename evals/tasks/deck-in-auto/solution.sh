python3 tools/deck.py init deck.json --title "Cells for Kids"
python3 tools/deck.py add deck.json --json '{"layout": "cards", "title": "Cells", "items": ["Tiny parts", "Each has a job"]}'
python3 tools/deck.py add deck.json --json '{"layout": "cards", "title": "Feelings", "items": ["All okay", "Talk about them"]}'
python3 tools/deck.py add deck.json --json '{"layout": "cards", "title": "Helpers", "items": ["Doctors", "Family"]}'
python3 tools/deck.py render deck.json pages/
