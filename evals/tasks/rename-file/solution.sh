mv helpers.py text_utils.py
sed -i 's/\bhelpers\b/text_utils/g' posts.py test_posts.py
